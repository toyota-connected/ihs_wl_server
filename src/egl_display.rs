// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Buffers from EGL implementations that share them over a protocol of their
//! own (feature `egl-wl-display`).
//!
//! Some Wayland EGL implementations neither use linux-dmabuf nor document
//! the protocol they use instead: the compositor is expected to call
//! `eglBindWaylandDisplayWL` (EGL_WL_bind_wayland_display) on its
//! libwayland-server display, and the EGL library serves the protocol
//! itself. Their clients fail eglInitialize otherwise. With this feature the
//! server runs on libwayland-server, binds an EGL display to it -- a display
//! only: no context, nothing rendered -- and turns each such wl_buffer into
//! a dma-buf through libgbm (GBM_BO_IMPORT_WL_BUFFER), which the shell then
//! imports like any other.

use std::collections::HashMap;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::{DisplayHandle, Resource, Weak};

/// The bound EGL display and the buffers it has shared, as dma-bufs.
#[derive(Default)]
pub struct EglBuffers {
    #[cfg(feature = "egl-wl-display")]
    bound: Option<bound::Bound>,
    cache: HashMap<ObjectId, (Weak<WlBuffer>, Dmabuf)>,
}

impl EglBuffers {
    /// Bind an EGL display to the server's display, when the feature is on
    /// and the EGL implementation and libgbm can.
    #[cfg_attr(not(feature = "egl-wl-display"), allow(unused_variables))]
    pub fn bind(dh: &DisplayHandle) -> Self {
        EglBuffers {
            #[cfg(feature = "egl-wl-display")]
            bound: bound::Bound::new(dh),
            cache: HashMap::new(),
        }
    }

    /// The dma-buf behind @p buffer, when the EGL implementation made it.
    pub fn dmabuf(&mut self, buffer: &WlBuffer) -> Option<Dmabuf> {
        if let Some((_, dmabuf)) = self.cache.get(&buffer.id()) {
            return Some(dmabuf.clone());
        }
        #[cfg(feature = "egl-wl-display")]
        {
            let dmabuf = self.bound.as_mut()?.import(buffer)?;
            self.cache
                .insert(buffer.id(), (buffer.downgrade(), dmabuf.clone()));
            Some(dmabuf)
        }
        #[cfg(not(feature = "egl-wl-display"))]
        None
    }

    /// Forget buffers their clients destroyed: their keys, to retire.
    pub fn reap(&mut self) -> Vec<ObjectId> {
        let dead: Vec<ObjectId> = self
            .cache
            .iter()
            .filter(|(_, (weak, _))| weak.upgrade().is_err())
            .map(|(id, _)| id.clone())
            .collect();
        for id in &dead {
            self.cache.remove(id);
        }
        dead
    }
}

#[cfg(feature = "egl-wl-display")]
mod bound {
    use std::os::fd::OwnedFd;

    use smithay::backend::allocator::dmabuf::{Dmabuf, DmabufFlags};
    use smithay::backend::allocator::gbm::GbmDevice;
    use smithay::backend::allocator::{Fourcc, Modifier};
    use smithay::backend::egl::display::EGLBufferReader;
    use smithay::backend::egl::EGLDisplay;
    use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
    use smithay::reexports::wayland_server::{DisplayHandle, Resource};
    use smithay::utils::DeviceFd;

    use crate::vendor_gbm::{Layout, VendorGbm, DRM_FORMAT_MOD_QCOM_COMPRESSED};

    pub struct Bound {
        // Dropped in this order: the reader unbinds before the display goes.
        _reader: EGLBufferReader,
        _display: EGLDisplay,
        gbm: VendorGbm,
        warned_compressed: bool,
    }

    impl Bound {
        pub fn new(dh: &DisplayHandle) -> Option<Self> {
            let gbm = VendorGbm::open()?;
            let (path, node) = crate::nodes::render_nodes("IHS_WL_STAGING_DEVICE").next()?;
            let device = GbmDevice::new(DeviceFd::from(OwnedFd::from(node)))
                .map_err(|e| tracing::info!(?path, "gbm: {e}"))
                .ok()?;
            // SAFETY: nothing else in this process terminates this display.
            let display = unsafe { EGLDisplay::new(device) }
                .map_err(|e| tracing::info!(?path, "EGL display: {e}"))
                .ok()?;
            let reader = display
                .bind_wl_display(dh)
                .map_err(|e| tracing::info!("eglBindWaylandDisplayWL: {e}"))
                .ok()?;
            tracing::info!(?path, "EGL display bound to the Wayland display");
            Some(Bound {
                _reader: reader,
                _display: display,
                gbm,
                warned_compressed: false,
            })
        }

        pub fn import(&mut self, buffer: &WlBuffer) -> Option<Dmabuf> {
            let ptr = buffer.id().as_ptr();
            if ptr.is_null() {
                return None;
            }
            // SAFETY: a live wl_buffer of the display the EGL implementation
            // is bound to.
            let (fd, layout) = match unsafe { self.gbm.import_wl_buffer(ptr.cast()) } {
                Ok(imported) => imported,
                Err(e) => {
                    tracing::debug!("EGL wl_buffer: {e}");
                    return None;
                }
            };
            if layout.modifier == DRM_FORMAT_MOD_QCOM_COMPRESSED && !self.warned_compressed {
                // Imported by the dma-buf alone, a compressed buffer can
                // decode wrongly: some state (fast clears) lives outside it.
                // The EGL implementation's switch for uncompressed shared
                // buffers is the way out.
                tracing::warn!(
                    "EGL client buffers are compressed and may show corrupted; \
                     have the client's EGL share them uncompressed"
                );
                self.warned_compressed = true;
            }
            dmabuf(fd, layout)
        }
    }

    fn dmabuf(fd: OwnedFd, layout: Layout) -> Option<Dmabuf> {
        let mut builder = Dmabuf::builder(
            (layout.width as i32, layout.height as i32),
            Fourcc::try_from(layout.format).ok()?,
            Modifier::from(layout.modifier),
            DmabufFlags::empty(),
        );
        builder.add_plane(fd, 0, layout.offset, layout.stride);
        builder.build()
    }
}
