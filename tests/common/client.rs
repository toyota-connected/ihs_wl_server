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
    wl_buffer, wl_callback, wl_compositor, wl_region, wl_registry, wl_shm, wl_shm_pool,
    wl_subcompositor, wl_subsurface, wl_surface,
};
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::wp::commit_timing::v1::client::{
    wp_commit_timer_v1, wp_commit_timing_manager_v1,
};
use wayland_protocols::wp::fifo::v1::client::{wp_fifo_manager_v1, wp_fifo_v1};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};
use wayland_protocols::wp::linux_drm_syncobj::v1::client::{
    wp_linux_drm_syncobj_manager_v1, wp_linux_drm_syncobj_surface_v1,
    wp_linux_drm_syncobj_timeline_v1,
};
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

#[derive(Default)]
struct App {
    compositor: Option<wl_compositor::WlCompositor>,
    subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    shm: Option<wl_shm::WlShm>,
    dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    dmabuf_version: u32,
    /// Default feedback's main_device, once its done arrived.
    main_device: Option<u64>,
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
}

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
        self._surface = Some(surface);
        self._toplevel = Some(toplevel);
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
        self._surface = Some(surface);
        self._toplevel = Some(toplevel);
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
delegate_noop!(App: ignore wl_surface::WlSurface);
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
