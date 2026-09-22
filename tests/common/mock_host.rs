//! A stand-in for the shell's platform-view registry, installed with
//! `ihs_pv_set_host` so the module runs against the real libihs_shared
//! without a shell or a GPU. It records factory registration and plays the
//! registry's side of a view's lifecycle: create (factory), then dispose.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::Mutex;

use ihs_wl_server::ffi::ihs::sys;

/// Mirrors `IhsPvHost` in ihs/platform_view_host.h, which is not installed.
#[repr(C)]
pub struct IhsPvHost {
    struct_size: usize,
    user_data: *mut c_void,
    register_factory: Option<
        unsafe extern "C" fn(*mut c_void, *const c_char, sys::IhsPvFactory, *mut c_void) -> c_int,
    >,
    unregister_factory: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
    query_capabilities:
        Option<unsafe extern "C" fn(*mut c_void, *mut sys::IhsPvCapabilities) -> c_int>,
    vulkan_context: Option<unsafe extern "C" fn(*mut c_void, *mut sys::IhsVulkanContext) -> c_int>,
    egl_context: Option<unsafe extern "C" fn(*mut c_void, *mut sys::IhsEglContext) -> c_int>,
    grant: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *mut sys::IhsPlatformView,
            u32,
            *const sys::IhsFormatModifier,
            *mut u32,
            *mut c_int,
            *mut usize,
        ) -> c_int,
    >,
    revoke: Option<unsafe extern "C" fn(*mut c_void, *mut sys::IhsPlatformView)>,
    grant_drm_plane_id: Option<unsafe extern "C" fn(*mut c_void, *mut sys::IhsPlatformView) -> u32>,
    grant_shm_fd:
        Option<unsafe extern "C" fn(*mut c_void, *mut sys::IhsPlatformView, *mut usize) -> c_int>,
    submit: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *mut sys::IhsPlatformView,
            *const sys::IhsFrame,
            c_int,
            *mut c_int,
        ) -> c_int,
    >,
    assets_path: Option<unsafe extern "C" fn(*mut c_void) -> *const c_char>,
}

unsafe impl Sync for IhsPvHost {}

extern "C" {
    fn ihs_pv_set_host(host: *const IhsPvHost);
}

struct Factory {
    factory: sys::IhsPvFactory,
    user_data: usize,
}

struct LiveView {
    callbacks: sys::IhsPvCallbacks,
    user_data: usize,
}

#[derive(Default)]
struct Registry {
    factories: HashMap<String, Factory>,
    views: HashMap<i32, LiveView>,
    grants: u32,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    f(REGISTRY
        .lock()
        .unwrap()
        .get_or_insert_with(Registry::default))
}

unsafe extern "C" fn register_factory(
    _ud: *mut c_void,
    view_type: *const c_char,
    factory: sys::IhsPvFactory,
    factory_ud: *mut c_void,
) -> c_int {
    let name = CStr::from_ptr(view_type).to_string_lossy().into_owned();
    with(|r| {
        r.factories.insert(
            name,
            Factory {
                factory,
                user_data: factory_ud as usize,
            },
        )
    });
    0
}

unsafe extern "C" fn unregister_factory(_ud: *mut c_void, view_type: *const c_char) {
    let name = CStr::from_ptr(view_type).to_string_lossy().into_owned();
    with(|r| r.factories.remove(&name));
}

unsafe extern "C" fn query_capabilities(
    _ud: *mut c_void,
    out: *mut sys::IhsPvCapabilities,
) -> c_int {
    static KEY: &CStr = c"mock";
    (*out).backend_key = KEY.as_ptr();
    (*out).kinds = sys::IHS_PV_KIND_TEXTURE_DMABUF_IMPORT | sys::IHS_PV_KIND_SOFTWARE_SHM;
    (*out).explicit_sync = 1;
    0
}

unsafe extern "C" fn grant(
    _ud: *mut c_void,
    _view: *mut sys::IhsPlatformView,
    _kind: u32,
    _format: *const sys::IhsFormatModifier,
    _plane: *mut u32,
    _shm_fd: *mut c_int,
    _shm_stride: *mut usize,
) -> c_int {
    with(|r| r.grants += 1);
    0
}

static HOST: IhsPvHost = IhsPvHost {
    struct_size: std::mem::size_of::<IhsPvHost>(),
    user_data: std::ptr::null_mut(),
    register_factory: Some(register_factory),
    unregister_factory: Some(unregister_factory),
    query_capabilities: Some(query_capabilities),
    vulkan_context: None,
    egl_context: None,
    grant: Some(grant),
    revoke: None,
    grant_drm_plane_id: None,
    grant_shm_fd: None,
    submit: None,
    assets_path: None,
};

pub fn install() {
    with(|r| *r = Registry::default());
    unsafe { ihs_pv_set_host(&HOST) };
}

pub fn uninstall() {
    unsafe { ihs_pv_set_host(std::ptr::null()) };
}

pub fn has_factory(view_type: &str) -> bool {
    with(|r| r.factories.contains_key(view_type))
}

pub fn grants() -> u32 {
    with(|r| r.grants)
}

/// Stand-in for a fake `IhsPlatformView*`; the module never dereferences it.
fn fake_view(id: i32) -> *mut sys::IhsPlatformView {
    (0x1000 + id as usize) as *mut sys::IhsPlatformView
}

/// What the registry does on a Flutter `create`: invoke the factory.
pub fn create_view(view_type: &str, id: i32, width: f64, height: f64) -> c_int {
    let (factory, fud) = with(|r| {
        let f = r
            .factories
            .get(view_type)
            .expect("no factory for view type");
        (f.factory.unwrap(), f.user_data)
    });
    let ty = CString::new(view_type).unwrap();
    let info = sys::IhsPvCreateInfo {
        struct_size: std::mem::size_of::<sys::IhsPvCreateInfo>(),
        id,
        view_type: ty.as_ptr(),
        width,
        height,
        ..Default::default()
    };
    let mut callbacks = sys::IhsPvCallbacks::default();
    let mut user_data: *mut c_void = std::ptr::null_mut();
    // Called without the registry lock held: the factory may call back in.
    let rc = unsafe {
        factory(
            &info,
            fud as *mut c_void,
            fake_view(id),
            &mut callbacks,
            &mut user_data,
        )
    };
    if rc == 0 {
        with(|r| {
            r.views.insert(
                id,
                LiveView {
                    callbacks,
                    user_data: user_data as usize,
                },
            )
        });
    }
    rc
}

pub fn resize_view(id: i32, width: f64, height: f64) {
    let (cb, ud) = with(|r| {
        let v = &r.views[&id];
        (v.callbacks.resize, v.user_data)
    });
    if let Some(resize) = cb {
        unsafe { resize(ud as *mut c_void, width, height) };
    }
}

/// What the registry does on a Flutter `dispose`.
pub fn dispose_view(id: i32) {
    let view = with(|r| r.views.remove(&id)).expect("no such view");
    if let Some(dispose) = view.callbacks.dispose {
        unsafe { dispose(view.user_data as *mut c_void) };
    }
}
