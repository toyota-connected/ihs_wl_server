// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! A minimal xdg-shell client: the in-tree equivalent of weston-simple-shm
//! (one toplevel, one ARGB shm buffer) and of weston-simple-dmabuf (a
//! toplevel, optionally with a subsurface, over linux-dmabuf buffers).
//!
//! Its "dma-bufs" are memfds. The server never touches a buffer's pixels --
//! it only dups the fds and hands them to the shell -- so any fd serves.

use std::collections::HashMap;
use std::fs::File;
use std::os::fd::{AsFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_pointer, wl_region, wl_registry,
    wl_seat, wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface, wl_surface, wl_touch,
};
use wayland_client::Proxy;
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::wp::commit_timing::v1::client::{
    wp_commit_timer_v1, wp_commit_timing_manager_v1,
};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1,
};
use wayland_protocols::wp::fifo::v1::client::{wp_fifo_manager_v1, wp_fifo_v1};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};
use wayland_protocols::wp::linux_drm_syncobj::v1::client::{
    wp_linux_drm_syncobj_manager_v1, wp_linux_drm_syncobj_surface_v1,
    wp_linux_drm_syncobj_timeline_v1,
};
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1;
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};

#[derive(Default)]
struct App {
    compositor: Option<wl_compositor::WlCompositor>,
    subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    shm: Option<wl_shm::WlShm>,
    dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    dmabuf_version: u32,
    /// Default feedback's main_device, once its done arrived.
    main_device: Option<u64>,
    /// The toplevel surface's dma-buf feedback: its tranches, as of each
    /// done, and the one being received.
    surface_feedback: Vec<Vec<Tranche>>,
    pending_surface_feedback: SurfaceFeedback,
    pending_main_device: Option<u64>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    globals: Vec<String>,
    configured: bool,
    /// The size of the toplevel's last configure.
    configure_size: Option<(i32, i32)>,
    /// wl_buffer.release events, by the client's buffer index.
    released: HashMap<usize, u32>,
    frames_done: u32,
    presentation: Option<wp_presentation::WpPresentation>,
    presentation_clock: Option<u32>,
    fifo_manager: Option<wp_fifo_manager_v1::WpFifoManagerV1>,
    timing_manager: Option<wp_commit_timing_manager_v1::WpCommitTimingManagerV1>,
    syncobj_manager: Option<wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1>,
    /// Presentation feedback outcomes, by the tag they were asked with.
    feedback: HashMap<usize, Feedback>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    touch: Option<wl_touch::WlTouch>,
    /// Input events, in arrival order.
    input: Vec<Input>,
    fractional_manager: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    /// The toplevel surface's last wp_fractional_scale_v1.preferred_scale.
    preferred_scale: Option<u32>,
    /// The toplevel surface's last wl_surface.preferred_buffer_scale.
    buffer_scale: Option<i32>,
    /// wl_surface.enter events, by surface protocol id.
    entered: HashMap<u32, u32>,
    toplevel_surface: u32,
    /// The serial of the last wl_pointer.button.
    button_serial: Option<u32>,
    /// The serial of the last wl_pointer.enter.
    enter_serial: Option<u32>,
    cursor_shape_manager: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    activation: Option<xdg_activation_v1::XdgActivationV1>,
    single_pixel: Option<wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1>,
    /// The last xdg_activation_token_v1.done.
    client_token: Option<String>,
    /// The popup's last xdg_popup.configure: x, y, width, height.
    popup_geometry: Option<(i32, i32, i32, i32)>,
    popup_done: bool,
    gbm_backend: Option<super::gbm_client::gbm_buffer_backend::GbmBufferBackend>,
    /// The last gbm_buffer_params outcome: the buffer, or None for failed.
    gbm_outcome: Option<Option<wl_buffer::WlBuffer>>,
}

/// A seat event, with the surface it names as its protocol id.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Enter {
        surface: u32,
        x: f64,
        y: f64,
    },
    Leave {
        surface: u32,
    },
    Motion {
        x: f64,
        y: f64,
    },
    Button {
        button: u32,
        pressed: bool,
    },
    Axis {
        horizontal: bool,
        value: f64,
    },
    AxisSource(u32),
    AxisValue120 {
        horizontal: bool,
        value120: i32,
    },
    Frame,
    Keymap,
    RepeatInfo {
        rate: i32,
        delay: i32,
    },
    KeyEnter {
        surface: u32,
    },
    KeyLeave {
        surface: u32,
    },
    Key {
        key: u32,
        pressed: bool,
    },
    Modifiers {
        depressed: u32,
    },
    TouchDown {
        surface: u32,
        id: i32,
        x: f64,
        y: f64,
    },
    TouchUp {
        id: i32,
    },
    TouchMotion {
        id: i32,
        x: f64,
        y: f64,
    },
    TouchFrame,
    TouchCancel,
}

/// A dma-buf feedback tranche: target device, scanout flag, and its
/// (fourcc, modifier) pairs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tranche {
    pub device: u64,
    pub scanout: bool,
    pub formats: Vec<(u32, u64)>,
}

/// A surface feedback being received.
#[derive(Default)]
struct SurfaceFeedback {
    table: Vec<(u32, u64)>,
    tranches: Vec<Tranche>,
    tranche: Option<Tranche>,
    indices: Vec<u16>,
}

/// Marks a surface's feedback object, as opposed to the default one.
struct OfSurface;

/// What a wp_presentation_feedback came to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Feedback {
    Presented {
        time_ns: u64,
        refresh_ns: u32,
        msc: u64,
        flags: u32,
    },
    Discarded,
}

/// DRM_FORMAT_XRGB8888.
pub const XRGB8888: u32 = 0x3432_5258;

pub struct Client {
    conn: Connection,
    queue: EventQueue<App>,
    app: App,
    _surface: Option<wl_surface::WlSurface>,
    _toplevel: Option<xdg_toplevel::XdgToplevel>,
    xdg: Option<xdg_surface::XdgSurface>,
    fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    popup: Option<(
        wl_surface::WlSurface,
        xdg_surface::XdgSurface,
        xdg_popup::XdgPopup,
    )>,
    viewport: Option<wp_viewport::WpViewport>,
    /// dma-buf buffers, by index, and the memfds behind them.
    buffers: Vec<Option<(wl_buffer::WlBuffer, File)>>,
    sub_surface: Option<(wl_surface::WlSurface, wl_subsurface::WlSubsurface)>,
    fifo: Option<wp_fifo_v1::WpFifoV1>,
    timer: Option<wp_commit_timer_v1::WpCommitTimerV1>,
    next_feedback: usize,
    timeline: Option<wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1>,
    syncobj_surface: Option<wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1>,
}

impl Client {
    pub fn connect(socket_name: &str) -> Client {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        let stream = UnixStream::connect(PathBuf::from(dir).join(socket_name)).unwrap();
        let conn = Connection::from_socket(stream).unwrap();
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut app = App::default();
        queue.roundtrip(&mut app).unwrap();
        Client {
            conn,
            queue,
            app,
            _surface: None,
            _toplevel: None,
            xdg: None,
            fractional: None,
            popup: None,
            viewport: None,
            buffers: Vec::new(),
            sub_surface: None,
            fifo: None,
            timer: None,
            next_feedback: 0,
            timeline: None,
            syncobj_surface: None,
        }
    }

    pub fn globals(&self) -> &[String] {
        &self.app.globals
    }

    /// Create a toplevel, wait for its configure, attach a buffer, commit.
    pub fn map_toplevel(&mut self, app_id: &str, title: &str, width: i32, height: i32) {
        let qh = self.queue.handle();
        let surface = self
            .app
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&qh, ());
        let xdg = self
            .app
            .wm_base
            .as_ref()
            .unwrap()
            .get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_app_id(app_id.into());
        toplevel.set_title(title.into());
        surface.commit();
        while !self.app.configured {
            self.queue.blocking_dispatch(&mut self.app).unwrap();
        }

        let stride = width * 4;
        let size = (stride * height) as usize;
        let file = memfd(size);
        let pool = self
            .app
            .shm
            .as_ref()
            .unwrap()
            .create_pool(file.as_fd(), size as i32, &qh, ());
        let buffer =
            pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, &qh, ());
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
        self.queue.roundtrip(&mut self.app).unwrap();
        self.app.toplevel_surface = surface.id().protocol_id();
        self._surface = Some(surface);
        self._toplevel = Some(toplevel);
        self.xdg = Some(xdg);
    }

    pub fn roundtrip(&mut self) {
        self.queue.roundtrip(&mut self.app).unwrap();
    }

    /// Create and configure a toplevel with no buffer yet.
    pub fn create_toplevel(&mut self, app_id: &str, title: &str) {
        let qh = self.queue.handle();
        let surface = self
            .app
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&qh, ());
        let xdg = self
            .app
            .wm_base
            .as_ref()
            .unwrap()
            .get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_app_id(app_id.into());
        toplevel.set_title(title.into());
        surface.commit();
        while !self.app.configured {
            self.queue.blocking_dispatch(&mut self.app).unwrap();
        }
        self.app.toplevel_surface = surface.id().protocol_id();
        self._surface = Some(surface);
        self._toplevel = Some(toplevel);
        self.xdg = Some(xdg);
    }

    /// A linux-dmabuf buffer of @p width x @p height XRGB8888 over a memfd.
    /// Returns its index.
    pub fn new_dmabuf(&mut self, width: i32, height: i32) -> usize {
        let file = memfd((width * 4 * height) as usize);
        self.dmabuf_over(file, width, height)
    }

    /// A "dma-buf" still being rendered into: its plane is the read end of a
    /// pipe, which polls ready -- the rendering done -- only once something
    /// is written to the returned write end. Returns its index.
    pub fn new_pending_dmabuf(&mut self, width: i32, height: i32) -> (usize, File) {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        let (read, write) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
        (self.dmabuf_over(read, width, height), write)
    }

    fn dmabuf_over(&mut self, file: File, width: i32, height: i32) -> usize {
        let qh = self.queue.handle();
        let stride = width * 4;
        let params = self
            .app
            .dmabuf
            .as_ref()
            .expect("no linux-dmabuf")
            .create_params(&qh, ());
        params.add(file.as_fd(), 0, 0, stride as u32, 0, 0);
        let index = self.buffers.len();
        let buffer = params.create_immed(
            width,
            height,
            XRGB8888,
            zwp_linux_buffer_params_v1::Flags::empty(),
            &qh,
            index,
        );
        params.destroy();
        self.buffers.push(Some((buffer, file)));
        index
    }

    /// Attach buffer @p index to the toplevel (whole, at 0,0), optionally
    /// asking for a frame callback, and commit.
    pub fn commit_buffer(&mut self, index: usize, frame: bool) {
        let qh = self.queue.handle();
        let surface = self._surface.as_ref().unwrap();
        let (buffer, _) = self.buffers[index].as_ref().unwrap();
        surface.attach(Some(buffer), 0, 0);
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        if frame {
            surface.frame(&qh, ());
        }
        surface.commit();
        self.roundtrip();
    }

    /// Attach a fresh @p width x @p height ARGB shm buffer, every pixel
    /// @p argb, to the toplevel and commit.
    pub fn commit_shm_buffer(&mut self, width: i32, height: i32, argb: u32) {
        self.commit_shm_damaged(width, height, argb, (0, 0, width, height));
    }

    /// commit_shm_buffer, damaging only @p damage (x, y, w, h).
    pub fn commit_shm_damaged(
        &mut self,
        width: i32,
        height: i32,
        argb: u32,
        damage: (i32, i32, i32, i32),
    ) {
        use std::io::Write;
        let qh = self.queue.handle();
        let stride = width * 4;
        let size = (stride * height) as usize;
        let mut file = memfd(size);
        let pixels: Vec<u8> = std::iter::repeat_n(argb.to_le_bytes(), (width * height) as usize)
            .flatten()
            .collect();
        file.write_all(&pixels).unwrap();
        let pool = self
            .app
            .shm
            .as_ref()
            .unwrap()
            .create_pool(file.as_fd(), size as i32, &qh, ());
        let buffer =
            pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, &qh, ());
        let surface = self._surface.as_ref().unwrap();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(damage.0, damage.1, damage.2, damage.3);
        surface.commit();
        self.roundtrip();
    }

    /// Commit the toplevel with nothing new attached.
    pub fn commit_no_attach(&mut self) {
        self._surface.as_ref().unwrap().commit();
        self.roundtrip();
    }

    /// Remove the toplevel's buffer and commit.
    pub fn commit_no_buffer(&mut self) {
        let surface = self._surface.as_ref().unwrap();
        surface.attach(None, 0, 0);
        surface.commit();
        self.roundtrip();
    }

    /// Mark the whole toplevel surface opaque at its next commit.
    pub fn set_opaque(&mut self, width: i32, height: i32) {
        let qh = self.queue.handle();
        let region = self.app.compositor.as_ref().unwrap().create_region(&qh, ());
        region.add(0, 0, width, height);
        self._surface
            .as_ref()
            .unwrap()
            .set_opaque_region(Some(&region));
        region.destroy();
    }

    /// A desynchronized subsurface at (@p x, @p y) showing buffer @p index.
    pub fn add_subsurface(&mut self, index: usize, x: i32, y: i32) {
        let qh = self.queue.handle();
        let parent = self._surface.as_ref().unwrap();
        let surface = self
            .app
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&qh, ());
        let sub =
            self.app
                .subcompositor
                .as_ref()
                .unwrap()
                .get_subsurface(&surface, parent, &qh, ());
        sub.set_position(x, y);
        sub.set_desync();
        let (buffer, _) = self.buffers[index].as_ref().unwrap();
        surface.attach(Some(buffer), 0, 0);
        surface.commit();
        // The position applies with the parent's next commit.
        parent.commit();
        self.roundtrip();
        self.sub_surface = Some((surface, sub));
    }

    /// Destroy buffer @p index.
    pub fn destroy_buffer(&mut self, index: usize) {
        if let Some((buffer, _)) = self.buffers[index].take() {
            buffer.destroy();
        }
        self.roundtrip();
    }

    /// wl_buffer.release events buffer @p index has had.
    pub fn released(&self, index: usize) -> u32 {
        self.app.released.get(&index).copied().unwrap_or(0)
    }

    pub fn frames_done(&self) -> u32 {
        self.app.frames_done
    }

    pub fn configure_size(&self) -> Option<(i32, i32)> {
        self.app.configure_size
    }

    /// Dispatch until @p done holds, or give up after a few seconds.
    pub fn dispatch_until(&mut self, what: &str, done: impl Fn(&Client) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !done(self) {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            self.roundtrip();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

fn memfd(size: usize) -> File {
    let fd = unsafe { libc::memfd_create(c"ihs-wl-test".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0);
    let file = unsafe { File::from_raw_fd(fd) };
    file.set_len(size as u64).unwrap();
    file
}

impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(
        app: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    app.compositor = Some(registry.bind(name, version.min(6), qh, ()))
                }
                "wl_shm" => app.shm = Some(registry.bind(name, 1, qh, ())),
                "wl_subcompositor" => app.subcompositor = Some(registry.bind(name, 1, qh, ())),
                "zwp_linux_dmabuf_v1" => {
                    app.dmabuf_version = version;
                    app.dmabuf = Some(registry.bind(name, version.min(4), qh, ()))
                }
                "xdg_wm_base" => app.wm_base = Some(registry.bind(name, 1, qh, ())),
                "wp_presentation" => {
                    app.presentation = Some(registry.bind(name, version.min(2), qh, ()))
                }
                "wp_fifo_manager_v1" => app.fifo_manager = Some(registry.bind(name, 1, qh, ())),
                "wp_linux_drm_syncobj_manager_v1" => {
                    app.syncobj_manager = Some(registry.bind(name, 1, qh, ()))
                }
                "wp_commit_timing_manager_v1" => {
                    app.timing_manager = Some(registry.bind(name, 1, qh, ()))
                }
                "wl_seat" => app.seat = Some(registry.bind(name, version.min(9), qh, ())),
                "gbm_buffer_backend" => app.gbm_backend = Some(registry.bind(name, 1, qh, ())),
                "wp_fractional_scale_manager_v1" => {
                    app.fractional_manager = Some(registry.bind(name, 1, qh, ()))
                }
                "wp_viewporter" => app.viewporter = Some(registry.bind(name, 1, qh, ())),
                "xdg_activation_v1" => app.activation = Some(registry.bind(name, 1, qh, ())),
                "wp_single_pixel_buffer_manager_v1" => {
                    app.single_pixel = Some(registry.bind(name, 1, qh, ()))
                }
                "wp_cursor_shape_manager_v1" => {
                    app.cursor_shape_manager = Some(registry.bind(name, 1, qh, ()))
                }
                "wl_output" => {
                    registry.bind::<wayland_client::protocol::wl_output::WlOutput, _, _>(
                        name,
                        version.min(4),
                        qh,
                        (),
                    );
                }
                _ => {}
            }
            app.globals.push(interface);
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for App {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for App {
    fn event(
        app: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            app.configured = true;
        }
    }
}

delegate_noop!(App: ignore wl_compositor::WlCompositor);
delegate_noop!(App: ignore wayland_client::protocol::wl_output::WlOutput);
delegate_noop!(App: ignore wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);
delegate_noop!(App: ignore wp_viewporter::WpViewporter);
delegate_noop!(App: ignore wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
delegate_noop!(App: ignore xdg_activation_v1::XdgActivationV1);
delegate_noop!(App: ignore wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1);

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            app.client_token = Some(token);
        }
    }
}
delegate_noop!(App: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);
delegate_noop!(App: ignore wp_viewport::WpViewport);

impl Dispatch<wl_surface::WlSurface, ()> for App {
    fn event(
        app: &mut Self,
        surface: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::Enter { .. } => {
                *app.entered.entry(surface.id().protocol_id()).or_default() += 1;
            }
            // Only the toplevel's is kept: the one the tests ask after.
            wl_surface::Event::PreferredBufferScale { factor }
                if surface.id().protocol_id() == app.toplevel_surface =>
            {
                app.buffer_scale = Some(factor);
            }
            _ => {}
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            app.preferred_scale = Some(scale);
        }
    }
}
delegate_noop!(App: ignore wl_shm::WlShm);
delegate_noop!(App: ignore wl_shm_pool::WlShmPool);
delegate_noop!(App: ignore wl_buffer::WlBuffer);
delegate_noop!(App: ignore wl_region::WlRegion);
delegate_noop!(App: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(App: ignore wl_subsurface::WlSubsurface);
delegate_noop!(App: ignore zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1);
delegate_noop!(App: ignore zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1);

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for App {
    fn event(
        app: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            app.configure_size = Some((width, height));
        }
    }
}

/// A dma-buf buffer, identified by its index.
impl Dispatch<wl_buffer::WlBuffer, usize> for App {
    fn event(
        app: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            *app.released.entry(*index).or_default() += 1;
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            app.frames_done += 1;
        }
    }
}

impl Client {
    fn surface_of(&self, sub: bool) -> &wl_surface::WlSurface {
        if sub {
            &self.sub_surface.as_ref().expect("no subsurface").0
        } else {
            self._surface.as_ref().unwrap()
        }
    }

    /// Ask for presentation feedback on the next commit of the toplevel
    /// (or, with @p sub, of the subsurface). Returns its tag.
    pub fn request_feedback(&mut self, sub: bool) -> usize {
        let qh = self.queue.handle();
        let tag = self.next_feedback;
        self.next_feedback += 1;
        let presentation = self.app.presentation.as_ref().expect("no wp_presentation");
        presentation.feedback(self.surface_of(sub), &qh, tag);
        tag
    }

    pub fn feedback(&self, tag: usize) -> Option<Feedback> {
        self.app.feedback.get(&tag).copied()
    }

    /// The clock wp_presentation announced.
    pub fn presentation_clock(&self) -> Option<u32> {
        self.app.presentation_clock
    }

    /// Attach buffer @p index to the subsurface and commit it.
    pub fn commit_sub_buffer(&mut self, index: usize) {
        let (buffer, _) = self.buffers[index].as_ref().unwrap();
        let surface = &self.sub_surface.as_ref().expect("no subsurface").0;
        surface.attach(Some(buffer), 0, 0);
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        surface.commit();
        self.roundtrip();
    }

    fn fifo(&mut self) -> &wp_fifo_v1::WpFifoV1 {
        if self.fifo.is_none() {
            let qh = self.queue.handle();
            let manager = self
                .app
                .fifo_manager
                .as_ref()
                .expect("no wp_fifo_manager_v1");
            self.fifo = Some(manager.get_fifo(self._surface.as_ref().unwrap(), &qh, ()));
        }
        self.fifo.as_ref().unwrap()
    }

    /// The next commit of the toplevel sets a fifo barrier.
    pub fn fifo_set_barrier(&mut self) {
        self.fifo().set_barrier();
    }

    /// The next commit of the toplevel waits for the fifo barrier.
    pub fn fifo_wait_barrier(&mut self) {
        self.fifo().wait_barrier();
    }

    /// The next commit of the toplevel targets CLOCK_MONOTONIC @p ns.
    pub fn set_target(&mut self, ns: u64) {
        if self.timer.is_none() {
            let qh = self.queue.handle();
            let manager = self
                .app
                .timing_manager
                .as_ref()
                .expect("no wp_commit_timing_manager_v1");
            self.timer = Some(manager.get_timer(self._surface.as_ref().unwrap(), &qh, ()));
        }
        let secs = ns / 1_000_000_000;
        self.timer.as_ref().unwrap().set_timestamp(
            (secs >> 32) as u32,
            secs as u32,
            (ns % 1_000_000_000) as u32,
        );
    }

    pub fn has_syncobj(&self) -> bool {
        self.app.syncobj_manager.is_some()
    }

    /// Use explicit sync on the toplevel, over the timeline @p fd.
    pub fn use_explicit_sync(&mut self, fd: std::os::fd::OwnedFd) {
        let qh = self.queue.handle();
        let manager = self
            .app
            .syncobj_manager
            .as_ref()
            .expect("no syncobj manager");
        self.timeline = Some(manager.import_timeline(fd.as_fd(), &qh, ()));
        self.syncobj_surface = Some(manager.get_surface(self._surface.as_ref().unwrap(), &qh, ()));
    }

    /// The next commit of the toplevel is ready at @p acquire, and its
    /// buffer free again at @p release.
    pub fn set_sync_points(&mut self, acquire: u64, release: u64) {
        let timeline = self.timeline.as_ref().unwrap();
        let surface = self.syncobj_surface.as_ref().unwrap();
        surface.set_acquire_point(timeline, (acquire >> 32) as u32, acquire as u32);
        surface.set_release_point(timeline, (release >> 32) as u32, release as u32);
    }

    /// The advertised zwp_linux_dmabuf_v1 version.
    pub fn dmabuf_version(&self) -> u32 {
        self.app.dmabuf_version
    }

    /// The default feedback's main_device (v4 only).
    pub fn default_feedback_main_device(&mut self) -> Option<u64> {
        let qh = self.queue.handle();
        let _feedback = self.app.dmabuf.as_ref()?.get_default_feedback(&qh, ());
        self.dispatch_until("dma-buf feedback", |c| c.app.main_device.is_some());
        self.app.main_device
    }

    /// Ask for the toplevel surface's dma-buf feedback (v4 only).
    pub fn request_surface_feedback(&mut self) {
        let qh = self.queue.handle();
        let surface = self._surface.as_ref().unwrap();
        self.app
            .dmabuf
            .as_ref()
            .unwrap()
            .get_surface_feedback(surface, &qh, OfSurface);
    }

    /// Dispatch until the surface feedback has been sent @p count times;
    /// the tranches of the last.
    pub fn wait_surface_feedback(&mut self, count: usize) -> Vec<Tranche> {
        self.dispatch_until("surface feedback", |c| {
            c.app.surface_feedback.len() >= count
        });
        self.app.surface_feedback[count - 1].clone()
    }

    /// How many times the surface feedback has been sent so far.
    pub fn surface_feedback_count(&mut self) -> usize {
        self.roundtrip();
        self.app.surface_feedback.len()
    }

    /// True once the server has gone away.
    pub fn roundtrip_fails(&mut self) -> bool {
        self.queue.roundtrip(&mut self.app).is_err()
    }
}

delegate_noop!(App: ignore wp_fifo_manager_v1::WpFifoManagerV1);
delegate_noop!(App: ignore wp_fifo_v1::WpFifoV1);
delegate_noop!(App: ignore wp_commit_timing_manager_v1::WpCommitTimingManagerV1);
delegate_noop!(App: ignore wp_commit_timer_v1::WpCommitTimerV1);

impl Dispatch<wp_presentation::WpPresentation, ()> for App {
    fn event(
        app: &mut Self,
        _: &wp_presentation::WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            app.presentation_clock = Some(clk_id);
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, usize> for App {
    fn event(
        app: &mut Self,
        _: &wp_presentation_feedback::WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        tag: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let outcome = match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
            } => Feedback::Presented {
                time_ns: (((tv_sec_hi as u64) << 32) | tv_sec_lo as u64) * 1_000_000_000
                    + tv_nsec as u64,
                refresh_ns: refresh,
                msc: ((seq_hi as u64) << 32) | seq_lo as u64,
                flags: match flags {
                    wayland_client::WEnum::Value(k) => k.bits(),
                    wayland_client::WEnum::Unknown(v) => v,
                },
            },
            wp_presentation_feedback::Event::Discarded => Feedback::Discarded,
            _ => return,
        };
        app.feedback.insert(*tag, outcome);
    }
}

delegate_noop!(App: ignore wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1);
delegate_noop!(App: ignore wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1);
delegate_noop!(App: ignore wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1);

impl Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_dmabuf_feedback_v1::Event::MainDevice { device } => {
                let mut dev = [0u8; 8];
                let n = device.len().min(8);
                dev[..n].copy_from_slice(&device[..n]);
                app.pending_main_device = Some(u64::from_ne_bytes(dev));
            }
            zwp_linux_dmabuf_feedback_v1::Event::Done => {
                app.main_device = app.pending_main_device;
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, OfSurface> for App {
    fn event(
        app: &mut Self,
        _: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &OfSurface,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_linux_dmabuf_feedback_v1::{Event, TrancheFlags};
        let fb = &mut app.pending_surface_feedback;
        match event {
            Event::FormatTable { fd, size } => fb.table = read_format_table(fd, size),
            Event::TrancheTargetDevice { device } => {
                let mut dev = [0u8; 8];
                let n = device.len().min(8);
                dev[..n].copy_from_slice(&device[..n]);
                fb.tranche = Some(Tranche {
                    device: u64::from_ne_bytes(dev),
                    scanout: false,
                    formats: Vec::new(),
                });
            }
            Event::TrancheFlags { flags } => {
                let scanout = matches!(flags, wayland_client::WEnum::Value(f) if f.contains(TrancheFlags::Scanout));
                fb.tranche.as_mut().expect("flags before target").scanout = scanout;
            }
            Event::TrancheFormats { indices } => {
                fb.indices.extend(
                    indices
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| u16::from_ne_bytes(*b)),
                );
            }
            Event::TrancheDone => {
                let mut tranche = fb.tranche.take().expect("tranche_done alone");
                tranche.formats = fb.indices.drain(..).map(|i| fb.table[i as usize]).collect();
                fb.tranches.push(tranche);
            }
            Event::Done => {
                let tranches = std::mem::take(&mut fb.tranches);
                app.surface_feedback.push(tranches);
            }
            _ => {}
        }
    }
}

/// A feedback format table: 16-byte (fourcc, pad, modifier) entries.
fn read_format_table(fd: std::os::fd::OwnedFd, size: u32) -> Vec<(u32, u64)> {
    use std::os::unix::fs::FileExt;
    // The same open file each time it is sent: read at 0, not at its offset.
    let mut bytes = vec![0u8; size as usize];
    File::from(fd).read_exact_at(&mut bytes, 0).unwrap();
    bytes
        .as_chunks::<16>()
        .0
        .iter()
        .map(|e| {
            (
                u32::from_ne_bytes(e[0..4].try_into().unwrap()),
                u64::from_ne_bytes(e[8..16].try_into().unwrap()),
            )
        })
        .collect()
}

impl Client {
    /// The toplevel's and the subsurface's protocol ids.
    pub fn surface_id(&self, sub: bool) -> u32 {
        self.surface_of(sub).id().protocol_id()
    }

    /// Get whichever of pointer, keyboard and touch the seat has.
    pub fn use_seat(&mut self) {
        self.dispatch_until("seat capabilities", |c| {
            c.app.pointer.is_some() && c.app.keyboard.is_some() && c.app.touch.is_some()
        });
    }

    /// Input events so far, and forget them.
    pub fn take_input(&mut self) -> Vec<Input> {
        std::mem::take(&mut self.app.input)
    }

    /// Dispatch until the input events so far include @p want, in that
    /// order though not necessarily adjacent; return them all.
    pub fn wait_input(&mut self, what: &str, want: &[Input]) -> Vec<Input> {
        self.dispatch_until(what, |c| {
            let mut want = want.iter().peekable();
            for got in &c.app.input {
                if want.peek() == Some(&got) {
                    want.next();
                }
            }
            want.peek().is_none()
        });
        self.take_input()
    }

    /// Activate the toplevel with @p token, as a client launched with it in
    /// XDG_ACTIVATION_TOKEN does.
    pub fn activate(&mut self, token: &str) {
        let activation = self.app.activation.as_ref().expect("xdg_activation_v1");
        activation.activate(token.into(), self._surface.as_ref().unwrap());
        self.roundtrip();
    }

    /// A token of the client's own making, as for launching another client.
    pub fn client_token(&mut self) -> String {
        let qh = self.queue.handle();
        let activation = self.app.activation.as_ref().expect("xdg_activation_v1");
        let token = activation.get_activation_token(&qh, ());
        token.commit();
        self.dispatch_until("activation token", |c| c.app.client_token.is_some());
        token.destroy();
        self.app.client_token.take().unwrap()
    }

    /// Ask for cursor @p shape over the surface the pointer entered last.
    pub fn set_cursor_shape(&mut self, shape: wp_cursor_shape_device_v1::Shape) {
        let qh = self.queue.handle();
        let manager = self
            .app
            .cursor_shape_manager
            .as_ref()
            .expect("cursor shape");
        let device = manager.get_pointer(self.app.pointer.as_ref().unwrap(), &qh, ());
        device.set_shape(self.app.enter_serial.unwrap(), shape);
        device.destroy();
        self.roundtrip();
    }

    /// wl_pointer.set_cursor over the surface the pointer entered last:
    /// a surface of the client's own, or none to hide it.
    pub fn set_cursor_surface(&mut self, own: bool) {
        let qh = self.queue.handle();
        let surface = own.then(|| {
            self.app
                .compositor
                .as_ref()
                .unwrap()
                .create_surface(&qh, ())
        });
        self.app.pointer.as_ref().unwrap().set_cursor(
            self.app.enter_serial.unwrap(),
            surface.as_ref(),
            0,
            0,
        );
        self.roundtrip();
    }

    /// Set the toplevel's window geometry at its next commit, and commit.
    pub fn set_window_geometry(&mut self, x: i32, y: i32, width: i32, height: i32) {
        let xdg = self.xdg.as_ref().unwrap();
        xdg.set_window_geometry(x, y, width, height);
        self._surface.as_ref().unwrap().commit();
        self.roundtrip();
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for App {
    fn event(
        app: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: wayland_client::WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Pointer) && app.pointer.is_none() {
                app.pointer = Some(seat.get_pointer(qh, ()));
            }
            if caps.contains(wl_seat::Capability::Keyboard) && app.keyboard.is_none() {
                app.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            if caps.contains(wl_seat::Capability::Touch) && app.touch.is_none() {
                app.touch = Some(seat.get_touch(qh, ()));
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::WEnum::Value;
        let horizontal = |a: &wayland_client::WEnum<wl_pointer::Axis>| {
            matches!(a, Value(wl_pointer::Axis::HorizontalScroll))
        };
        let input = match event {
            wl_pointer::Event::Enter {
                surface,
                surface_x,
                surface_y,
                serial,
            } => {
                app.enter_serial = Some(serial);
                Input::Enter {
                    surface: surface.id().protocol_id(),
                    x: surface_x,
                    y: surface_y,
                }
            }
            wl_pointer::Event::Leave { surface, .. } => Input::Leave {
                surface: surface.id().protocol_id(),
            },
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => Input::Motion {
                x: surface_x,
                y: surface_y,
            },
            wl_pointer::Event::Button {
                button,
                state,
                serial,
                ..
            } => {
                app.button_serial = Some(serial);
                Input::Button {
                    button,
                    pressed: matches!(state, Value(wl_pointer::ButtonState::Pressed)),
                }
            }
            wl_pointer::Event::Axis { axis, value, .. } => Input::Axis {
                horizontal: horizontal(&axis),
                value,
            },
            wl_pointer::Event::AxisSource { axis_source } => Input::AxisSource(match axis_source {
                Value(s) => s as u32,
                wayland_client::WEnum::Unknown(v) => v,
            }),
            wl_pointer::Event::AxisValue120 { axis, value120 } => Input::AxisValue120 {
                horizontal: horizontal(&axis),
                value120,
            },
            wl_pointer::Event::Frame => Input::Frame,
            _ => return,
        };
        app.input.push(input);
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let input = match event {
            wl_keyboard::Event::Keymap { .. } => Input::Keymap,
            wl_keyboard::Event::RepeatInfo { rate, delay } => Input::RepeatInfo { rate, delay },
            wl_keyboard::Event::Enter { surface, .. } => Input::KeyEnter {
                surface: surface.id().protocol_id(),
            },
            wl_keyboard::Event::Leave { surface, .. } => Input::KeyLeave {
                surface: surface.id().protocol_id(),
            },
            wl_keyboard::Event::Key { key, state, .. } => Input::Key {
                key,
                pressed: matches!(
                    state,
                    wayland_client::WEnum::Value(wl_keyboard::KeyState::Pressed)
                ),
            },
            wl_keyboard::Event::Modifiers { mods_depressed, .. } => Input::Modifiers {
                depressed: mods_depressed,
            },
            _ => return,
        };
        app.input.push(input);
    }
}

impl Dispatch<wl_touch::WlTouch, ()> for App {
    fn event(
        app: &mut Self,
        _: &wl_touch::WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let input = match event {
            wl_touch::Event::Down {
                surface, id, x, y, ..
            } => Input::TouchDown {
                surface: surface.id().protocol_id(),
                id,
                x,
                y,
            },
            wl_touch::Event::Up { id, .. } => Input::TouchUp { id },
            wl_touch::Event::Motion { id, x, y, .. } => Input::TouchMotion { id, x, y },
            wl_touch::Event::Frame => Input::TouchFrame,
            wl_touch::Event::Cancel => Input::TouchCancel,
            _ => return,
        };
        app.input.push(input);
    }
}

impl Client {
    /// Ask for the toplevel surface's fractional scale.
    pub fn use_fractional_scale(&mut self) {
        let qh = self.queue.handle();
        let manager = self
            .app
            .fractional_manager
            .as_ref()
            .expect("no wp_fractional_scale_manager_v1");
        self.fractional =
            Some(manager.get_fractional_scale(self._surface.as_ref().unwrap(), &qh, ()));
        self.roundtrip();
    }

    /// The last preferred scale, in 120ths.
    pub fn preferred_scale(&self) -> Option<u32> {
        self.app.preferred_scale
    }

    /// The toplevel surface's last preferred buffer scale.
    pub fn buffer_scale(&self) -> Option<i32> {
        self.app.buffer_scale
    }

    /// wl_surface.enter events the surface has had.
    pub fn entered(&self, sub: bool) -> u32 {
        let id = self.surface_id(sub);
        self.app.entered.get(&id).copied().unwrap_or(0)
    }

    /// Attach a single-pixel buffer of premultiplied @p rgba (8 bits each) to
    /// the toplevel, and commit.
    pub fn commit_single_pixel(&mut self, rgba: [u8; 4]) {
        let qh = self.queue.handle();
        let manager = self
            .app
            .single_pixel
            .as_ref()
            .expect("single-pixel buffers");
        let wide = |c: u8| u32::from(c) * 0x0101_0101;
        let buffer = manager.create_u32_rgba_buffer(
            wide(rgba[0]),
            wide(rgba[1]),
            wide(rgba[2]),
            wide(rgba[3]),
            &qh,
            (),
        );
        let surface = self._surface.as_ref().unwrap();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 1, 1);
        surface.commit();
        self.roundtrip();
    }

    /// Show the toplevel at @p width x @p height logical pixels from its next
    /// commit on, whatever its buffer's size.
    pub fn set_viewport_destination(&mut self, width: i32, height: i32) {
        if self.viewport.is_none() {
            let qh = self.queue.handle();
            let viewporter = self.app.viewporter.as_ref().expect("no wp_viewporter");
            self.viewport = Some(viewporter.get_viewport(self._surface.as_ref().unwrap(), &qh, ()));
        }
        self.viewport
            .as_ref()
            .unwrap()
            .set_destination(width, height);
    }
}

/// Where a popup goes, relative to its parent's window geometry.
pub struct Placement {
    pub anchor: (i32, i32, i32, i32),
    pub size: (i32, i32),
    /// xdg_positioner.constraint_adjustment bits.
    pub adjust: u32,
}

impl Client {
    /// Open a popup of the toplevel as @p at says, over a @p size buffer,
    /// grabbing with the last button serial when @p grab. Waits for its
    /// configure, then maps it.
    pub fn open_popup(&mut self, at: Placement, grab: bool) {
        let qh = self.queue.handle();
        let wm_base = self.app.wm_base.as_ref().unwrap();
        let positioner = wm_base.create_positioner(&qh, ());
        positioner.set_size(at.size.0, at.size.1);
        let (x, y, w, h) = at.anchor;
        positioner.set_anchor_rect(x, y, w, h);
        positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
        positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
        positioner.set_constraint_adjustment(
            xdg_positioner::ConstraintAdjustment::from_bits_truncate(at.adjust),
        );
        let surface = self
            .app
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
        let popup = xdg.get_popup(self.xdg.as_ref(), &positioner, &qh, ());
        positioner.destroy();
        if grab {
            let serial = self
                .app
                .button_serial
                .expect("no button serial to grab with");
            popup.grab(self.app.seat.as_ref().unwrap(), serial);
        }
        self.app.popup_geometry = None;
        self.app.popup_done = false;
        surface.commit();
        self.dispatch_until("popup configure", |c| c.app.popup_geometry.is_some());
        let buffer = self.new_dmabuf(at.size.0, at.size.1);
        let (wl_buffer, _) = self.buffers[buffer].as_ref().unwrap();
        surface.attach(Some(wl_buffer), 0, 0);
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        surface.commit();
        self.roundtrip();
        self.popup = Some((surface, xdg, popup));
    }

    /// The popup's configured x, y, width and height.
    pub fn popup_geometry(&self) -> Option<(i32, i32, i32, i32)> {
        self.app.popup_geometry
    }

    pub fn popup_done(&self) -> bool {
        self.app.popup_done
    }

    pub fn popup_surface_id(&self) -> u32 {
        self.popup.as_ref().expect("no popup").0.id().protocol_id()
    }

    /// Destroy the popup, as a client does once it is done.
    pub fn close_popup(&mut self) {
        if let Some((surface, xdg, popup)) = self.popup.take() {
            popup.destroy();
            xdg.destroy();
            surface.destroy();
        }
        self.roundtrip();
    }
}

delegate_noop!(App: ignore xdg_positioner::XdgPositioner);

impl Dispatch<xdg_popup::XdgPopup, ()> for App {
    fn event(
        app: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => app.popup_geometry = Some((x, y, width, height)),
            xdg_popup::Event::PopupDone => app.popup_done = true,
            _ => {}
        }
    }
}

use super::gbm_client::{gbm_buffer_backend, gbm_buffer_params};

/// The udata of a wl_buffer gbm_buffer_params created.
pub struct GbmCreated;

impl Client {
    /// Share a @p width x @p height buffer of DRM @p format over
    /// gbm_buffer_backend, its metadata fd holding @p metadata. Returns its
    /// index, or None when the server said failed.
    pub fn gbm_buffer(
        &mut self,
        width: u32,
        height: u32,
        format: u32,
        flags: i32,
        metadata: &[u8],
    ) -> Option<usize> {
        let qh = self.queue.handle();
        let backend = self
            .app
            .gbm_backend
            .as_ref()
            .expect("no gbm_buffer_backend");
        let params = backend.create_params(&qh, ());
        let file = memfd((width * height * 4) as usize);
        let meta = memfd(metadata.len().max(1));
        std::os::unix::fs::FileExt::write_all_at(&meta, metadata, 0).unwrap();
        self.app.gbm_outcome = None;
        params.create(file.as_fd(), meta.as_fd(), width, height, format, flags);
        self.dispatch_until("gbm_buffer_params outcome", |c| c.app.gbm_outcome.is_some());
        params.destroy();
        let buffer = self.app.gbm_outcome.take().unwrap()?;
        self.buffers.push(Some((buffer, file)));
        Some(self.buffers.len() - 1)
    }

    /// Ask a used gbm_buffer_params for a second buffer; true when refused.
    pub fn gbm_params_reused_fails(&mut self) -> bool {
        let qh = self.queue.handle();
        let backend = self
            .app
            .gbm_backend
            .as_ref()
            .expect("no gbm_buffer_backend");
        let params = backend.create_params(&qh, ());
        let file = memfd(64 * 4);
        for _ in 0..2 {
            self.app.gbm_outcome = None;
            params.create(file.as_fd(), file.as_fd(), 8, 8, XRGB8888, 0);
            self.dispatch_until("gbm_buffer_params outcome", |c| c.app.gbm_outcome.is_some());
        }
        params.destroy();
        matches!(self.app.gbm_outcome, Some(None))
    }
}

delegate_noop!(App: ignore gbm_buffer_backend::GbmBufferBackend);
impl Dispatch<wl_buffer::WlBuffer, GbmCreated> for App {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Event,
        _: &GbmCreated,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<gbm_buffer_params::GbmBufferParams, ()> for App {
    fn event(
        app: &mut Self,
        _: &gbm_buffer_params::GbmBufferParams,
        event: gbm_buffer_params::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            gbm_buffer_params::Event::Created { buffer } => app.gbm_outcome = Some(Some(buffer)),
            gbm_buffer_params::Event::Failed => app.gbm_outcome = Some(None),
        }
    }

    wayland_client::event_created_child!(App, gbm_buffer_params::GbmBufferParams, [
        gbm_buffer_params::EVT_CREATED_OPCODE => (wl_buffer::WlBuffer, GbmCreated),
    ]);
}
