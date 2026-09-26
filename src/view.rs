// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! The `ihs_wl/toplevel` platform-view factory and per-view link.
//!
//! Dispose rule (platform_view.h header block): once `dispose` returns, no
//! `ihs_pv_submit*` for that view may be in flight or start. Each view's
//! `ViewLink` sits in a mutex that the compositor thread holds across every
//! submit/retire, and that `dispose` clears -- so dispose waits out an
//! in-flight submit and every later one finds `None`.

use std::ffi::{c_int, c_void, CStr};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use crate::ffi::ihs::sys;
use crate::observe::{self, Observed};
use crate::params::ViewParams;
use crate::thread::{self, Cmd};

pub const VIEW_TYPE: &CStr = c"ihs_wl/toplevel";

/// What the compositor thread needs to submit to a live view.
pub struct ViewLink {
    pub view: *mut sys::IhsPlatformView,
    pub grant: sys::IhsPvGrant,
}

// SAFETY: the registry allows ihs_pv_submit from any thread; the handle is
// only dereferenced by libihs_shared, and only under the ViewShared lock.
unsafe impl Send for ViewLink {}

pub struct ViewShared {
    /// The Flutter platform-view id, which also keys input.
    pub id: i32,
    /// What the widget passed in creationParams: what to bind to.
    pub params: ViewParams,
    /// Its size at creation in physical pixels, as `resize` reports it; the
    /// view is laid out before it is created, so no resize may follow.
    pub size: Option<(i32, i32)>,
    pub link: Mutex<Option<ViewLink>>,
}

pub type ViewHandle = Arc<ViewShared>;

impl std::fmt::Debug for ViewShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "View({})", self.id)
    }
}

pub fn register_factory() {
    // Platform-thread only by contract, but ihs_shared gives an FFI module no
    // way onto the platform thread, so this runs on the caller's thread -- as
    // every ihs_pv producer loaded from Dart does today. The shell's registry
    // mutex is what makes that safe.
    let rc = unsafe {
        sys::ihs_pv_register_factory(VIEW_TYPE.as_ptr(), Some(factory), std::ptr::null_mut())
    };
    if rc == sys::IHS_PV_OK as c_int {
        tracing::info!("registered platform-view factory {VIEW_TYPE:?}");
    } else if rc == sys::IHS_PV_ERR_NO_REGISTRY {
        tracing::info!("no platform-view registry (no shell host); clients connect but no views");
    } else {
        tracing::warn!(rc, "ihs_pv_register_factory: {}", ihs_last_error());
    }
}

pub fn unregister_factory() {
    unsafe { sys::ihs_pv_unregister_factory(VIEW_TYPE.as_ptr()) };
}

fn ihs_last_error() -> String {
    unsafe { CStr::from_ptr(sys::ihs_last_error_message()) }
        .to_string_lossy()
        .into_owned()
}

/// The requirement every view negotiates: zero-copy import with the SHM
/// floor, explicit sync preferred, composited inline with Flutter's layers.
fn requirements() -> sys::IhsPvRequirements {
    sys::IhsPvRequirements {
        struct_size: std::mem::size_of::<sys::IhsPvRequirements>(),
        kinds: sys::IHS_PV_KIND_TEXTURE_DMABUF_IMPORT | sys::IHS_PV_KIND_SOFTWARE_SHM,
        formats: std::ptr::null(),
        format_count: 0,
        needs_alpha: 1,
        sync: sys::IHS_PV_SYNC_EXPLICIT_PREFERRED as u8,
        z_order: sys::IHS_PV_Z_INLINE as u8,
        reserved: 0,
    }
}

unsafe extern "C" fn factory(
    info: *const sys::IhsPvCreateInfo,
    _factory_user_data: *mut c_void,
    view: *mut sys::IhsPlatformView,
    out_callbacks: *mut sys::IhsPvCallbacks,
    out_user_data: *mut *mut c_void,
) -> c_int {
    let result = panic::catch_unwind(AssertUnwindSafe(|| -> c_int {
        if info.is_null() || view.is_null() || out_callbacks.is_null() || out_user_data.is_null() {
            return sys::IHS_PV_ERR_INVALID;
        }
        let id = (*info).id;
        if !thread::is_running() {
            // Don't take a grant for a view nothing will ever feed.
            tracing::warn!(id, "view created with no server running");
            return sys::IHS_PV_ERR_NO_BACKEND;
        }

        let mut grant = sys::IhsPvGrant {
            struct_size: std::mem::size_of::<sys::IhsPvGrant>(),
            ..Default::default()
        };
        let rc = sys::ihs_pv_negotiate(view, &requirements(), &mut grant);
        if rc != sys::IHS_PV_OK as c_int {
            tracing::warn!(id, rc, "ihs_pv_negotiate: {}", ihs_last_error());
            return rc;
        }
        tracing::info!(
            id,
            kind = grant.granted_kind,
            sync = grant.sync,
            fourcc = grant.format.fourcc,
            "view created"
        );

        let params = if (*info).params.is_null() || (*info).params_size == 0 {
            ViewParams::default()
        } else {
            ViewParams::decode(std::slice::from_raw_parts(
                (*info).params,
                (*info).params_size,
            ))
        };
        // Created at its logical size (what the widget laid out).
        let dpr = params.dpr.unwrap_or(1.0);
        let (w, h) = (
            ((*info).width * dpr).round(),
            ((*info).height * dpr).round(),
        );
        let shared = Arc::new(ViewShared {
            id,
            params,
            size: (w >= 1.0 && h >= 1.0).then_some((w as i32, h as i32)),
            link: Mutex::new(Some(ViewLink { view, grant })),
        });
        if let Err(e) = thread::send(Cmd::ViewCreated(shared.clone())) {
            tracing::warn!(id, "view created with no server: {e}");
            return sys::IHS_PV_ERR_NO_BACKEND;
        }

        *out_callbacks = sys::IhsPvCallbacks {
            struct_size: std::mem::size_of::<sys::IhsPvCallbacks>(),
            resize: Some(on_resize),
            on_touch: None, // input comes straight from Dart over FFI
            accept_gesture: None,
            reject_gesture: None,
            set_suspended: Some(on_set_suspended),
            renegotiate: Some(on_renegotiate),
            dispose: Some(on_dispose),
            presented: Some(on_presented),
            scanout_hint: Some(on_scanout_hint),
        };
        *out_user_data = Arc::into_raw(shared) as *mut c_void;
        observe::emit(Observed::ViewCreated { view_id: id });
        sys::IHS_PV_OK as c_int
    }));
    result.unwrap_or_else(|p| {
        tracing::error!("factory panicked: {}", crate::panic_message(&*p));
        sys::IHS_PV_ERR_INVALID
    })
}

/// Borrow the view behind a callback's user_data.
unsafe fn shared<'a>(user_data: *mut c_void) -> &'a ViewShared {
    &*(user_data as *const ViewShared)
}

fn callback(name: &str, f: impl FnOnce()) {
    if let Err(p) = panic::catch_unwind(AssertUnwindSafe(f)) {
        tracing::error!("{name} panicked: {}", crate::panic_message(&*p));
    }
}

unsafe extern "C" fn on_resize(user_data: *mut c_void, width: f64, height: f64) {
    callback("resize", || {
        let view = shared(user_data);
        tracing::debug!(id = view.id, width, height, "resize");
        let _ = thread::send(Cmd::ViewResized {
            view_id: view.id,
            width: width.round() as i32,
            height: height.round() as i32,
        });
    });
}

/// The shell's display thread: a frame of this view reached the screen.
unsafe extern "C" fn on_presented(
    user_data: *mut c_void,
    seq: u64,
    ust_ns: u64,
    refresh_ns: u32,
    msc: u64,
    flags: u32,
) {
    callback("presented", || {
        let view = shared(user_data);
        let _ = thread::send(Cmd::Presented {
            view_id: view.id,
            report: crate::timing::Report {
                seq,
                ust_ns,
                refresh_ns,
                msc,
                flags,
            },
        });
    });
}

/// The shell's display thread: layer @p layer_id could go on a plane if it
/// were in one of @p formats.
unsafe extern "C" fn on_scanout_hint(
    user_data: *mut c_void,
    layer_id: u32,
    dev: u64,
    formats: *const sys::IhsFormatModifier,
    count: usize,
) {
    callback("scanout_hint", || {
        let view = shared(user_data);
        let formats = if formats.is_null() || count == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(formats, count)
                .iter()
                .map(|f| crate::caps::FormatModifier {
                    fourcc: f.fourcc,
                    modifier: f.modifier,
                })
                .collect()
        };
        let _ = thread::send(Cmd::ScanoutHint {
            view_id: view.id,
            hint: crate::scanout::Hint {
                layer_id,
                dev,
                formats,
            },
        });
    });
}

unsafe extern "C" fn on_set_suspended(user_data: *mut c_void, suspended: u8) {
    callback("set_suspended", || {
        let view = shared(user_data);
        tracing::debug!(id = view.id, suspended, "set_suspended");
        let _ = thread::send(Cmd::ViewSuspended {
            view_id: view.id,
            suspended: suspended != 0,
        });
    });
}

unsafe extern "C" fn on_renegotiate(user_data: *mut c_void) {
    callback("renegotiate", || {
        let view = shared(user_data);
        let mut link = view.link.lock().unwrap_or_else(|e| e.into_inner());
        let Some(link) = link.as_mut() else { return };
        let rc = sys::ihs_pv_negotiate(link.view, &requirements(), &mut link.grant);
        if rc == sys::IHS_PV_OK as c_int {
            tracing::info!(id = view.id, kind = link.grant.granted_kind, "renegotiated");
        } else {
            // The grant is zeroed (kind NONE); nothing is submitted until the
            // next successful renegotiate.
            tracing::warn!(id = view.id, rc, "renegotiate failed: {}", ihs_last_error());
        }
    });
}

unsafe extern "C" fn on_dispose(user_data: *mut c_void) {
    callback("dispose", || {
        // Takes back the reference the factory handed out.
        let view = Arc::from_raw(user_data as *const ViewShared);
        // Waits out an in-flight submit; every later one sees None.
        *view.link.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let _ = thread::send(Cmd::ViewDisposed(view.id));
        observe::emit(Observed::ViewDisposed { view_id: view.id });
        tracing::info!(id = view.id, "view disposed");
    });
}
