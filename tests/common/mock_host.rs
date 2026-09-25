// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! A stand-in for the shell's platform-view registry, installed with
//! `ihs_pv_set_host` so the module runs against the real libihs_shared
//! without a shell or a GPU. It records factory registration and plays the
//! registry's side of a view's lifecycle: create (factory), then dispose.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
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
    post_platform_task: Option<
        unsafe extern "C" fn(
            *mut c_void,
            Option<unsafe extern "C" fn(*mut c_void)>,
            *mut c_void,
        ) -> c_int,
    >,
    is_platform_thread: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    retire_buffer:
        Option<unsafe extern "C" fn(*mut c_void, *mut sys::IhsPlatformView, u32) -> c_int>,
    submit_layers: Option<
        unsafe extern "C" fn(
            *mut c_void,
            *mut sys::IhsPlatformView,
            *const sys::IhsLayer,
            usize,
            u64,
            *mut c_int,
        ) -> c_int,
    >,
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

/// One layer of a submission, as the registry received it.
#[derive(Clone, Debug, PartialEq)]
pub struct SubmittedLayer {
    pub layer_id: u32,
    pub buffer_id: u32,
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub plane_count: u32,
    /// 16.16
    pub src: (i32, i32, u32, u32),
    pub dst: (i32, i32, u32, u32),
    pub transform: u32,
    pub opaque: bool,
    /// The pixel at the probe point (set_probe), read from the buffer.
    pub probe: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Submission {
    pub view_id: i32,
    pub seq: u64,
    pub layers: Vec<SubmittedLayer>,
}

#[derive(Default)]
struct Registry {
    factories: HashMap<String, Factory>,
    views: HashMap<i32, LiveView>,
    grants: u32,
    submissions: Vec<Submission>,
    retired: Vec<(i32, u32)>,
    /// Hand back an eventfd per layer as its release fence, as the EGL
    /// backends do; otherwise none, as the Vulkan ones do.
    fenced: bool,
    /// Our side of the eventfds handed out: (buffer_id, fd).
    fences: Vec<(u32, OwnedFd)>,
    /// Read the pixel at (x, y) of each submitted buffer.
    probe: Option<(u32, u32)>,
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

fn view_id_of(view: *mut sys::IhsPlatformView) -> i32 {
    (view as usize - 0x1000) as i32
}

/// Close every plane fd of @p frame once, as the registry does.
/// The 32-bit pixel at (@p x, @p y) of @p frame's first plane, when it is a
/// mappable dma-buf (a real one, not the tests' memfds -- those map too).
unsafe fn read_pixel(frame: &sys::IhsFrame, x: u32, y: u32) -> Option<u32> {
    if frame.plane_count == 0 || x >= frame.width || y >= frame.height {
        return None;
    }
    let fd = frame.plane_fd[0];
    let off = frame.plane_offset[0] as usize + y as usize * frame.plane_stride[0] as usize;
    let len = off + (x as usize + 1) * 4;
    let ptr = libc::mmap(
        std::ptr::null_mut(),
        len,
        libc::PROT_READ,
        libc::MAP_SHARED,
        fd,
        0,
    );
    if ptr == libc::MAP_FAILED {
        return None;
    }
    // Wait for writes still in flight, as a consumer honoring the buffer's
    // implicit fence would (a gbm map may write back through the GPU).
    const DMA_BUF_IOCTL_SYNC: libc::c_ulong = 0x4008_6200;
    let sync = |flags: u64| libc::ioctl(fd, DMA_BUF_IOCTL_SYNC, &flags);
    sync(1); // START | READ
    let px = std::ptr::read_unaligned((ptr as *const u8).add(off + x as usize * 4) as *const u32);
    sync(1 | 4); // END | READ
    libc::munmap(ptr, len);
    Some(px)
}

unsafe fn consume_frame(frame: &sys::IhsFrame) {
    let n = (frame.plane_count as usize).min(4);
    for i in 0..n {
        let fd = frame.plane_fd[i];
        if fd >= 0 && !frame.plane_fd[..i].contains(&fd) {
            drop(OwnedFd::from_raw_fd(fd));
        }
    }
}

unsafe extern "C" fn submit_layers(
    _ud: *mut c_void,
    view: *mut sys::IhsPlatformView,
    layers: *const sys::IhsLayer,
    count: usize,
    seq: u64,
    out_fences: *mut c_int,
) -> c_int {
    // The registry takes NULL for no layers.
    let layers = if count == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(layers, count)
    };
    let mut recorded = Vec::new();
    for (i, l) in layers.iter().enumerate() {
        let f = &*l.frame;
        recorded.push(SubmittedLayer {
            layer_id: l.layer_id,
            buffer_id: f.buffer_id,
            width: f.width,
            height: f.height,
            fourcc: f.format.fourcc,
            modifier: f.format.modifier,
            plane_count: f.plane_count,
            src: (l.src_x, l.src_y, l.src_w, l.src_h),
            dst: (l.dst_x, l.dst_y, l.dst_w, l.dst_h),
            transform: l.transform,
            opaque: l.opaque != 0,
            probe: with(|r| r.probe).and_then(|(x, y)| read_pixel(f, x, y)),
        });
        consume_frame(f);
        if l.acquire_fence_fd >= 0 {
            drop(OwnedFd::from_raw_fd(l.acquire_fence_fd));
        }
        let out = if out_fences.is_null() {
            None
        } else {
            Some(&mut *out_fences.add(i))
        };
        if let Some(out) = out {
            *out = -1;
            let fenced = with(|r| r.fenced);
            if fenced {
                let efd = libc::eventfd(0, libc::EFD_CLOEXEC);
                assert!(efd >= 0);
                let ours = OwnedFd::from_raw_fd(efd);
                *out = libc::dup(ours.as_raw_fd());
                with(|r| r.fences.push((f.buffer_id, ours)));
            }
        }
    }
    with(|r| {
        r.submissions.push(Submission {
            view_id: view_id_of(view),
            seq,
            layers: recorded,
        })
    });
    0
}

unsafe extern "C" fn retire_buffer(
    _ud: *mut c_void,
    view: *mut sys::IhsPlatformView,
    buffer_id: u32,
) -> c_int {
    with(|r| r.retired.push((view_id_of(view), buffer_id)));
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
    post_platform_task: None,
    is_platform_thread: None,
    retire_buffer: Some(retire_buffer),
    submit_layers: Some(submit_layers),
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
    create_view_with_params(view_type, id, width, height, &[])
}

/// The same, with the widget's encoded creationParams.
pub fn create_view_with_params(
    view_type: &str,
    id: i32,
    width: f64,
    height: f64,
    params: &[u8],
) -> c_int {
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
        params: if params.is_empty() {
            std::ptr::null()
        } else {
            params.as_ptr()
        },
        params_size: params.len(),
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

/// Read the pixel at (@p x, @p y) of every buffer submitted from now on.
pub fn set_probe(at: Option<(u32, u32)>) {
    with(|r| r.probe = at);
}

/// Hand back an eventfd per layer as its release fence from now on.
pub fn set_fenced(fenced: bool) {
    with(|r| r.fenced = fenced);
}

/// Every submission so far.
pub fn submissions() -> Vec<Submission> {
    with(|r| r.submissions.clone())
}

/// Every (view, buffer_id) retired so far.
pub fn retired() -> Vec<(i32, u32)> {
    with(|r| r.retired.clone())
}

/// Signal the release fences handed out for @p buffer_id, as the shell does
/// once it is done with the buffer.
pub fn signal_release(buffer_id: u32) {
    let fences: Vec<OwnedFd> = with(|r| {
        let (hit, keep): (Vec<_>, Vec<_>) =
            r.fences.drain(..).partition(|(id, _)| *id == buffer_id);
        r.fences = keep;
        hit.into_iter().map(|(_, fd)| fd).collect()
    });
    for fd in fences {
        let one: u64 = 1;
        unsafe { libc::write(fd.as_raw_fd(), (&one as *const u64).cast(), 8) };
    }
}

/// What the display does once a frame of the view is on screen.
pub fn present(id: i32, seq: u64) {
    present_at(id, seq, 1_000_000_000 * seq, 16_666_667, seq, 0);
}

/// present(), with the report spelled out.
pub fn present_at(id: i32, seq: u64, ust_ns: u64, refresh_ns: u32, msc: u64, flags: u32) {
    let (cb, ud) = with(|r| {
        let v = &r.views[&id];
        (v.callbacks.presented, v.user_data)
    });
    if let Some(presented) = cb {
        unsafe { presented(ud as *mut c_void, seq, ust_ns, refresh_ns, msc, flags) };
    }
}

/// The view leaves (true) or re-enters the scene.
pub fn set_suspended(id: i32, suspended: bool) {
    let (cb, ud) = with(|r| {
        let v = &r.views[&id];
        (v.callbacks.set_suspended, v.user_data)
    });
    if let Some(set_suspended) = cb {
        unsafe { set_suspended(ud as *mut c_void, suspended as u8) };
    }
}
