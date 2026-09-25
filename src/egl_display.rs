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
//! server runs on libwayland-server and binds an EGL display to it -- a
//! display only: no context, nothing rendered.
//!
//! Which display, and what the shell is handed, depends on the shell:
//! - one that samples image layers (IHS_PV_KIND_TEXTURE_EGL_IMAGE, the EGL
//!   backends) gets each buffer as an EGLImage made on its own display. The
//!   driver keeps everything about the buffer, compression included.
//! - otherwise each buffer becomes a dma-buf through libgbm
//!   (GBM_BO_IMPORT_WL_BUFFER), bound on a display of the module's own. A
//!   dma-buf cannot carry every kind of compression metadata, so compressed
//!   buffers can show corrupted there.

use std::collections::HashMap;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::{DisplayHandle, Resource, Weak};

use crate::caps::Caps;

/// What a layer shows. Images only come with the `egl-wl-display` feature.
#[derive(Clone)]
#[cfg_attr(not(feature = "egl-wl-display"), allow(dead_code))]
pub enum Content {
    Dmabuf(Dmabuf),
    Image(Image),
}

/// An EGLImage on the shell's display, owned by the module until the buffer
/// behind it goes.
#[derive(Clone, Copy, Debug)]
pub struct Image {
    /// EGLImageKHR, as an address.
    pub egl_image: usize,
    pub width: u32,
    pub height: u32,
    /// Sampled as GL_TEXTURE_EXTERNAL_OES.
    pub external: bool,
    /// Stored bottom row first.
    pub y_inverted: bool,
}

#[cfg(feature = "egl-wl-display")]
type Keep = Option<smithay::backend::egl::EGLBuffer>;
#[cfg(not(feature = "egl-wl-display"))]
type Keep = ();

/// The bound EGL display and the buffers it has shared.
#[derive(Default)]
pub struct EglBuffers {
    #[cfg(feature = "egl-wl-display")]
    mode: Option<Mode>,
    /// Each buffer's content, and what keeps its image alive.
    cache: HashMap<ObjectId, (Weak<WlBuffer>, Content, Keep)>,
}

#[cfg(feature = "egl-wl-display")]
enum Mode {
    Shared(shared::Shared),
    Gbm(bound::Bound),
}

impl EglBuffers {
    /// Bind an EGL display to the server's display, when the feature is on
    /// and the EGL implementation can: the shell's, when it samples image
    /// layers, else one of the module's own.
    #[cfg_attr(not(feature = "egl-wl-display"), allow(unused_variables))]
    pub fn bind(dh: &DisplayHandle, caps: &Caps) -> Self {
        #[cfg(feature = "egl-wl-display")]
        let mode = caps
            .egl_images
            .and_then(|shell| shared::Shared::new(dh, shell))
            .map(Mode::Shared)
            .or_else(|| bound::Bound::new(dh).map(Mode::Gbm));
        EglBuffers {
            #[cfg(feature = "egl-wl-display")]
            mode,
            cache: HashMap::new(),
        }
    }

    /// What @p buffer shows, when the EGL implementation made it.
    pub fn content(&mut self, buffer: &WlBuffer) -> Option<Content> {
        if let Some((_, content, _)) = self.cache.get(&buffer.id()) {
            return Some(content.clone());
        }
        #[cfg(feature = "egl-wl-display")]
        {
            let (content, keep) = match self.mode.as_mut()? {
                Mode::Shared(shared) => shared.import(buffer)?,
                Mode::Gbm(bound) => (Content::Dmabuf(bound.import(buffer)?), None),
            };
            self.cache
                .insert(buffer.id(), (buffer.downgrade(), content.clone(), keep));
            Some(content)
        }
        #[cfg(not(feature = "egl-wl-display"))]
        None
    }

    /// Forget buffers their clients destroyed: their keys, to retire.
    pub fn reap(&mut self) -> Vec<ObjectId> {
        let dead: Vec<ObjectId> = self
            .cache
            .iter()
            .filter(|(_, (weak, _, _))| weak.upgrade().is_err())
            .map(|(id, _)| id.clone())
            .collect();
        for id in &dead {
            self.cache.remove(id);
        }
        dead
    }
}

#[cfg(feature = "egl-wl-display")]
mod shared {
    use smithay::backend::egl::display::EGLBufferReader;
    use smithay::backend::egl::{EGLBuffer, EGLDisplay, Format};
    use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
    use smithay::reexports::wayland_server::DisplayHandle;

    use super::{Content, Image};
    use crate::caps::ShellEgl;

    /// Bound to the shell's own display, handing it EGLImages.
    pub struct Shared {
        // Dropped in this order: the reader unbinds before the display goes.
        reader: EGLBufferReader,
        _display: EGLDisplay,
    }

    impl Shared {
        pub fn new(dh: &DisplayHandle, shell: ShellEgl) -> Option<Self> {
            // SAFETY: the shell's display and config, valid for the backend's
            // life; wrapped, not owned, so never terminated from here.
            let display = unsafe {
                EGLDisplay::from_raw(shell.display as *const _, shell.config as *const _)
            }
            .map_err(|e| tracing::info!("shell EGL display: {e}"))
            .ok()?;
            let reader = display
                .bind_wl_display(dh)
                .map_err(|e| tracing::info!("eglBindWaylandDisplayWL on the shell's display: {e}"))
                .ok()?;
            tracing::info!("shell EGL display bound to the Wayland display; buffers go as images");
            Some(Shared {
                reader,
                _display: display,
            })
        }

        pub fn import(&self, buffer: &WlBuffer) -> Option<(Content, Option<EGLBuffer>)> {
            let egl = match self.reader.egl_buffer_contents(buffer) {
                Ok(egl) => egl,
                Err(e) => {
                    tracing::debug!("EGL wl_buffer: {e}");
                    return None;
                }
            };
            let image = Image {
                egl_image: egl.image(0)? as usize,
                width: egl.size.w.max(0) as u32,
                height: egl.size.h.max(0) as u32,
                external: matches!(egl.format, Format::External),
                y_inverted: egl.y_inverted,
            };
            Some((Content::Image(image), Some(egl)))
        }
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
