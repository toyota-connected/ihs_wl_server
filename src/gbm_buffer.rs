// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! gbm_buffer_backend: buffers shared as a gbm buffer rather than through
//! linux-dmabuf.
//!
//! Some Wayland EGL implementations hand the compositor each buffer as an fd
//! plus a metadata fd describing it, over `gbm_buffer_backend`
//! (protocols/gbm-buffer-backend.xml, from the CodeLinaro weston fork,
//! git.codelinaro.org/clo/le/wayland/weston, commit 52547d20). Such a client
//! refuses to start without it. The protocol carries no stride or modifier:
//! the libgbm that allocated the buffer works them out from the metadata
//! (CodeLinaro libgbm, git.codelinaro.org/clo/le/display/libgbm). So the
//! buffer is imported through that libgbm, and what comes back is an
//! ordinary dma-buf -- the same wl_buffer a linux-dmabuf client gets, which
//! everything downstream already handles, every plane of it (a video
//! decoder's NV12 has two) on the one fd.
//!
//! The global is only offered when the loaded libgbm is one that can import
//! such buffers: it exports `gbm_perform`, which Mesa's does not.

use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufFlags};
use smithay::backend::allocator::{Buffer as _, Fourcc, Modifier};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
};

use crate::state::State;

#[allow(
    non_upper_case_globals,
    non_camel_case_types,
    missing_docs,
    clippy::all
)]
pub mod protocol {
    use smithay::reexports::wayland_server;
    use smithay::reexports::wayland_server::protocol::*;

    pub mod __interfaces {
        use smithay::reexports::wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/gbm-buffer-backend.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("protocols/gbm-buffer-backend.xml");
}

use protocol::gbm_buffer_backend::{self, GbmBufferBackend};
use protocol::gbm_buffer_params::{self, GbmBufferParams};

pub use crate::vendor_gbm::{Plane, Planes};

/// Works out a shared buffer's layout from its fd and metadata fd.
pub trait Import: Send + Sync {
    fn layout(
        &self,
        fd: &OwnedFd,
        metadata: OwnedFd,
        width: u32,
        height: u32,
        format: u32,
    ) -> Result<Planes, String>;
}

static TEST_IMPORT: Mutex<Option<Arc<dyn Import>>> = Mutex::new(None);

/// Stand in for libgbm, for tests: servers started from now on offer the
/// global and import through @p import. None goes back to libgbm.
#[doc(hidden)]
pub fn set_test_import(import: Option<Arc<dyn Import>>) {
    *TEST_IMPORT.lock().unwrap_or_else(|e| e.into_inner()) = import;
}

/// The importer for this process, if any: a test's, else libgbm when it can.
pub fn importer() -> Option<Arc<dyn Import>> {
    if let Some(import) = TEST_IMPORT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return Some(import);
    }
    crate::vendor_gbm::VendorGbm::open().map(|g| Arc::new(g) as Arc<dyn Import>)
}

/// State of the global.
pub struct GbmBufferState {
    import: Arc<dyn Import>,
}

impl GbmBufferState {
    /// Offer the global, when there is anything to import with.
    pub fn new(dh: &DisplayHandle) -> Option<Self> {
        let import = importer()?;
        dh.create_global::<State, GbmBufferBackend, ()>(1, ());
        tracing::info!("gbm_buffer_backend offered");
        Some(GbmBufferState { import })
    }
}

/// A params object, until its one create.
#[derive(Default)]
pub struct Params {
    used: Mutex<bool>,
}

/// gbm_buffer_params.flags.y_invert.
const Y_INVERT: i32 = 1;

impl GlobalDispatch<GbmBufferBackend, ()> for State {
    fn bind(
        _state: &mut Self,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<GbmBufferBackend>,
        _data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<GbmBufferBackend, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &GbmBufferBackend,
        request: gbm_buffer_backend::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            gbm_buffer_backend::Request::CreateParams { params_id } => {
                data_init.init(params_id, Params::default());
            }
            gbm_buffer_backend::Request::Destroy => {}
        }
    }
}

impl Dispatch<GbmBufferParams, Params> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        params: &GbmBufferParams,
        request: gbm_buffer_params::Request,
        data: &Params,
        dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let gbm_buffer_params::Request::Create {
            fd,
            meta_fd,
            width,
            height,
            format,
            flags,
        } = request
        else {
            return;
        };
        // One buffer per params object, as upstream weston has it.
        if std::mem::replace(&mut *data.used.lock().unwrap(), true) {
            params.failed();
            return;
        }
        let Some(gbm) = state.gbm_buffer.as_ref() else {
            params.failed();
            return;
        };
        match import(&*gbm.import, fd, meta_fd, width, height, format, flags) {
            Ok(dmabuf) => {
                if !state.caps.accepts(format, dmabuf.format().modifier.into()) {
                    tracing::warn!(
                        format,
                        modifier = u64::from(dmabuf.format().modifier),
                        "gbm buffer the shell does not list as importable"
                    );
                }
                match client.create_resource::<WlBuffer, Dmabuf, State>(dh, 1, dmabuf) {
                    Ok(buffer) => params.created(&buffer),
                    Err(e) => {
                        tracing::warn!("gbm buffer wl_buffer: {e}");
                        params.failed();
                    }
                }
            }
            Err(e) => {
                tracing::warn!(width, height, format, "gbm buffer import: {e}");
                params.failed();
            }
        }
    }
}

/// The dma-buf behind a shared gbm buffer: every plane, on the one fd.
fn import(
    import: &dyn Import,
    fd: OwnedFd,
    metadata: OwnedFd,
    width: u32,
    height: u32,
    format: u32,
    flags: i32,
) -> Result<Dmabuf, String> {
    let fourcc = Fourcc::try_from(format).map_err(|_| format!("unknown format {format:#x}"))?;
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(format!("bad size {width}x{height}"));
    }
    let layout = import.layout(&fd, metadata, width, height, format)?;
    if layout.planes.is_empty() || layout.planes.len() > 4 {
        return Err(format!("{} planes", layout.planes.len()));
    }
    let mut builder = Dmabuf::builder(
        (width as i32, height as i32),
        fourcc,
        Modifier::from(layout.modifier),
        if flags & Y_INVERT != 0 {
            DmabufFlags::Y_INVERT
        } else {
            DmabufFlags::empty()
        },
    );
    for (i, plane) in layout.planes.iter().enumerate() {
        let plane_fd = fd.try_clone().map_err(|e| format!("dup: {e}"))?;
        builder.add_plane(plane_fd, i as u32, plane.offset, plane.stride);
    }
    builder.build().ok_or_else(|| "no dma-buf".into())
}

impl Import for crate::vendor_gbm::VendorGbm {
    fn layout(
        &self,
        fd: &OwnedFd,
        metadata: OwnedFd,
        width: u32,
        height: u32,
        format: u32,
    ) -> Result<Planes, String> {
        self.import_gbm_buf(fd, metadata, width, height, format)
    }
}
