// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! The layout of a buffer a libgbm with vendor extensions describes.
//!
//! CodeLinaro libgbm (git.codelinaro.org/clo/le/display/libgbm) keeps a
//! buffer's layout in a metadata buffer beside it, and reports compression
//! only through `gbm_perform`: `gbm_bo_get_modifier` says linear either way.
//! Only used when the loaded libgbm exports `gbm_perform`, which Mesa's does
//! not.

use std::ffi::{c_int, c_void};
use std::os::fd::{AsRawFd, OwnedFd};

/// A buffer's only plane.
#[cfg(feature = "egl-wl-display")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub offset: u32,
    pub stride: u32,
    pub modifier: u64,
}

// From CodeLinaro libgbm's gbm_priv.h and gbm.h.
const GBM_PERFORM_GET_UBWC_STATUS: c_int = 0x19;
const GBM_PERFORM_GET_PLANE_INFO: c_int = 0x29;
const GBM_BO_IMPORT_GBM_BUF_TYPE: u32 = 0x4FFF;
/// gbm_priv.h MAX_NUM_OF_PLANES.
const MAX_NUM_OF_PLANES: usize = 5;
#[cfg(feature = "egl-wl-display")]
const GBM_BO_IMPORT_WL_BUFFER: u32 = 0x5501;
const GBM_BO_USE_RENDERING: u32 = 1 << 2;
/// drm_fourcc.h DRM_FORMAT_MOD_QCOM_COMPRESSED.
pub(crate) const DRM_FORMAT_MOD_QCOM_COMPRESSED: u64 = (0x05 << 56) | 1;
const DRM_FORMAT_MOD_LINEAR: u64 = 0;

type Perform = unsafe extern "C" fn(operation: c_int, ...) -> c_int;

/// gbm_priv.h struct gbm_buf_info.
#[repr(C)]
struct GbmBufInfo {
    fd: c_int,
    metadata_fd: c_int,
    width: u32,
    height: u32,
    format: u32,
}

/// gbm_priv.h generic_plane_t.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct GenericPlane {
    top_left: usize,
    offset: u32,
    component_id: i32,
    aligned_width: u32,
    aligned_height: u32,
    size: u32,
    bits_per_component: i32,
    bits_used: i32,
    h_increment: i32,
    v_increment: i32,
    h_subsampling: i32,
    v_subsampling: i32,
    stride: u32,
}

/// gbm_priv.h generic_buf_layout_t.
#[repr(C)]
#[derive(Default)]
struct GenericBufLayout {
    pixel_format: c_int,
    num_planes: u32,
    planes: [GenericPlane; MAX_NUM_OF_PLANES],
}

/// One plane of a buffer, within its one dma-buf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plane {
    pub offset: u32,
    pub stride: u32,
}

/// A buffer's planes, and the modifier its metadata implies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planes {
    pub modifier: u64,
    pub planes: Vec<Plane>,
}

/// The loaded libgbm, on a render node of its own.
pub struct VendorGbm {
    device: *mut gbm_sys::gbm_device,
    perform: Perform,
    _node: std::fs::File,
}

// SAFETY: only used from the compositor thread; the device pointer never
// leaves this struct.
unsafe impl Send for VendorGbm {}
unsafe impl Sync for VendorGbm {}

impl VendorGbm {
    /// The loaded libgbm, when it has the vendor extensions.
    pub fn open() -> Option<Self> {
        // SAFETY: a lookup by name in what is already loaded.
        let perform = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"gbm_perform".as_ptr()) };
        if perform.is_null() {
            return None;
        }
        // SAFETY: gbm_perform has this signature in every libgbm exporting it.
        let perform: Perform = unsafe { std::mem::transmute::<*mut c_void, Perform>(perform) };
        for (path, node) in crate::nodes::render_nodes("IHS_WL_STAGING_DEVICE") {
            // SAFETY: the node outlives the device, which Drop destroys first.
            let device = unsafe { gbm_sys::gbm_create_device(node.as_raw_fd()) };
            if !device.is_null() {
                tracing::debug!(?path, "vendor libgbm on");
                return Some(VendorGbm {
                    device,
                    perform,
                    _node: node,
                });
            }
        }
        None
    }

    #[cfg(feature = "egl-wl-display")]
    /// The dma-buf behind a wl_buffer the EGL implementation created, and
    /// its layout.
    ///
    /// # Safety
    /// @p resource is a live `struct wl_resource *` of that wl_buffer, on the
    /// display the EGL implementation is bound to.
    pub unsafe fn import_wl_buffer(
        &self,
        resource: *mut c_void,
    ) -> Result<(OwnedFd, Layout), String> {
        let bo = gbm_sys::gbm_bo_import(
            self.device,
            GBM_BO_IMPORT_WL_BUFFER,
            resource,
            GBM_BO_USE_RENDERING,
        );
        if bo.is_null() {
            return Err("gbm_bo_import(wl_buffer) failed".into());
        }
        // This libgbm hands back the bo's own fd rather than a new one: keep
        // a dup past the bo.
        let raw = gbm_sys::gbm_bo_get_fd(bo);
        let fd = if raw >= 0 {
            let dup = libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 0);
            (dup >= 0).then(|| <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(dup))
        } else {
            None
        };
        let mut ubwc: c_int = 0;
        let rc = (self.perform)(GBM_PERFORM_GET_UBWC_STATUS, bo, &mut ubwc as *mut c_int);
        let layout = Layout {
            width: gbm_sys::gbm_bo_get_width(bo),
            height: gbm_sys::gbm_bo_get_height(bo),
            format: gbm_sys::gbm_bo_get_format(bo),
            offset: gbm_sys::gbm_bo_get_offset(bo, 0),
            stride: gbm_sys::gbm_bo_get_stride(bo),
            modifier: if ubwc != 0 {
                DRM_FORMAT_MOD_QCOM_COMPRESSED
            } else {
                DRM_FORMAT_MOD_LINEAR
            },
        };
        gbm_sys::gbm_bo_destroy(bo);
        let fd = fd.ok_or("no fd")?;
        if rc != 0 || layout.stride == 0 {
            return Err(format!("layout: rc {rc}, stride {}", layout.stride));
        }
        Ok((fd, layout))
    }
}

impl VendorGbm {
    /// The layout of a buffer shared as an fd and a metadata fd
    /// (gbm_buffer_backend): every plane, from the metadata.
    pub fn import_gbm_buf(
        &self,
        fd: &OwnedFd,
        metadata: OwnedFd,
        width: u32,
        height: u32,
        format: u32,
    ) -> Result<Planes, String> {
        // The bo owns what it imports and closes it when destroyed: give it a
        // dup, and the metadata fd, which nothing else needs.
        let dup = fd.try_clone().map_err(|e| format!("dup: {e}"))?;
        let mut info = GbmBufInfo {
            fd: std::os::fd::IntoRawFd::into_raw_fd(dup),
            metadata_fd: std::os::fd::IntoRawFd::into_raw_fd(metadata),
            width,
            height,
            format,
        };
        // SAFETY: info matches gbm_buf_info and lives across the call.
        let bo = unsafe {
            gbm_sys::gbm_bo_import(
                self.device,
                GBM_BO_IMPORT_GBM_BUF_TYPE,
                &mut info as *mut GbmBufInfo as *mut c_void,
                GBM_BO_USE_RENDERING,
            )
        };
        if bo.is_null() {
            // SAFETY: not taken over by a bo, so still ours.
            unsafe {
                libc::close(info.fd);
                libc::close(info.metadata_fd);
            }
            return Err("gbm_bo_import(gbm_buf_info) failed".into());
        }
        let mut ubwc: c_int = 0;
        let mut layout = GenericBufLayout::default();
        // SAFETY: bo is live until destroyed below.
        let (rc, info_rc, stride, offset) = unsafe {
            let rc = (self.perform)(GBM_PERFORM_GET_UBWC_STATUS, bo, &mut ubwc as *mut c_int);
            let info_rc = (self.perform)(
                GBM_PERFORM_GET_PLANE_INFO,
                bo,
                &mut layout as *mut GenericBufLayout,
            );
            let stride = gbm_sys::gbm_bo_get_stride(bo);
            let offset = gbm_sys::gbm_bo_get_offset(bo, 0);
            gbm_sys::gbm_bo_destroy(bo);
            (rc, info_rc, stride, offset)
        };
        if rc != 0 {
            return Err(format!("UBWC status: {rc}"));
        }
        let n = layout.num_planes as usize;
        let planes = if info_rc == 0 && (1..=MAX_NUM_OF_PLANES).contains(&n) {
            for (i, p) in layout.planes[..n].iter().enumerate() {
                tracing::debug!(
                    i,
                    offset = p.offset,
                    stride = p.stride,
                    v_increment = p.v_increment,
                    aligned_width = p.aligned_width,
                    aligned_height = p.aligned_height,
                    size = p.size,
                    "gbm buffer plane"
                );
            }
            // The row pitch, as upstream weston takes it.
            layout.planes[..n]
                .iter()
                .map(|p| Plane {
                    offset: p.offset,
                    stride: p.v_increment.max(0) as u32,
                })
                .collect()
        } else {
            vec![Plane { offset, stride }]
        };
        if planes.iter().any(|p| p.stride == 0) {
            return Err("no stride".into());
        }
        Ok(Planes {
            modifier: if ubwc != 0 {
                DRM_FORMAT_MOD_QCOM_COMPRESSED
            } else {
                DRM_FORMAT_MOD_LINEAR
            },
            planes,
        })
    }
}

impl Drop for VendorGbm {
    fn drop(&mut self) {
        // SAFETY: created in open, destroyed once.
        unsafe { gbm_sys::gbm_device_destroy(self.device) };
    }
}
