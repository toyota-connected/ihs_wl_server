//! A minimal xdg-shell + wl_shm client, the in-tree equivalent of
//! weston-simple-shm: map one toplevel with one ARGB buffer.

use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

#[derive(Default)]
struct App {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    globals: Vec<String>,
    configured: bool,
}

pub struct Client {
    conn: Connection,
    queue: EventQueue<App>,
    app: App,
    _surface: Option<wl_surface::WlSurface>,
    _toplevel: Option<xdg_toplevel::XdgToplevel>,
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

    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

fn memfd(size: usize) -> File {
    use std::os::fd::FromRawFd;
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
                "xdg_wm_base" => app.wm_base = Some(registry.bind(name, 1, qh, ())),
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
delegate_noop!(App: ignore xdg_toplevel::XdgToplevel);

impl Client {
    /// True once the server has gone away.
    pub fn roundtrip_fails(&mut self) -> bool {
        self.queue.roundtrip(&mut self.app).is_err()
    }
}
