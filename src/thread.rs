// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! The compositor thread and its process-global handle.
//!
//! One thread owns the calloop loop, the `Display` and all smithay state.
//! Everything else talks to it by sending a `Cmd` down a calloop channel, so
//! no FFI entry point ever blocks on the compositor.

use std::ffi::{c_char, CString};
use std::os::unix::ffi::OsStrExt;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{mpsc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{channel, EventLoop, Interest, Mode, PostAction};
use smithay::reexports::wayland_server::{BindError, Display};
use smithay::wayland::socket::ListeningSocketSource;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::state::State;

/// Requests from FFI entry points to the compositor thread.
#[derive(Debug)]
pub enum Cmd {
    Quit,
    /// A view was created by the factory (platform thread).
    ViewCreated(crate::view::ViewHandle),
    /// A view's dispose callback ran; its link is already cleared.
    ViewDisposed(i32),
    /// The view was laid out at this size, in physical pixels.
    ViewResized {
        view_id: i32,
        width: i32,
        height: i32,
    },
    /// The shell showed a frame of the view (display thread).
    Presented {
        view_id: i32,
        report: crate::timing::Report,
    },
    /// The view left (true) or re-entered the scene.
    ViewSuspended {
        view_id: i32,
        suspended: bool,
    },
    Pointer {
        view_id: i32,
        event: crate::IhsWlPointerEvent,
    },
    Touch {
        view_id: i32,
        event: crate::IhsWlTouchEvent,
    },
    Key {
        view_id: i32,
        evdev: u32,
        pressed: bool,
        time_us: u64,
    },
    Focus {
        view_id: i32,
        focused: bool,
    },
    /// The shell's scanout hint for a layer of the view (display thread).
    ScanoutHint {
        view_id: i32,
        hint: crate::scanout::Hint,
    },
}

struct Running {
    tx: channel::Sender<Cmd>,
    thread: JoinHandle<()>,
    socket_name: CString,
}

static SERVER: Mutex<Option<Running>> = Mutex::new(None);

/// Serializes start and stop end to end. Held across factory
/// (un)registration, which must not happen under `SERVER`: the factory
/// itself takes `SERVER` (via `send`) on the platform thread.
static LIFECYCLE: Mutex<()> = Mutex::new(());

fn server() -> MutexGuard<'static, Option<Running>> {
    // A panic is never raised while this is held, but a poisoned lock must
    // not turn every later FFI call into one.
    SERVER.lock().unwrap_or_else(|e| e.into_inner())
}

type Ready = mpsc::Sender<Result<(channel::Sender<Cmd>, CString)>>;

pub fn start(config: Config) -> Result<()> {
    crate::log::init();
    let _lifecycle = LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner());
    let mut guard = server();
    if let Some(running) = guard.as_ref() {
        if !running.thread.is_finished() {
            // Hot restart re-runs start; the server, its clients and their
            // toplevels carry on. The new isolate re-creates its views, which
            // rebind by token or app_id.
            return Err(Error::new(
                crate::IhsWlResult::ErrAlreadyRunning as i32,
                "server already running",
            ));
        }
        // The compositor thread died. Reap it and start fresh.
        let dead = guard.take().unwrap();
        let _ = dead.thread.join();
    }

    // A capability query is platform-thread only, and this is the thread the
    // embedder starts the server from; the compositor thread gets the answer.
    let caps = crate::caps::query();

    let (ready_tx, ready_rx) = mpsc::channel();
    let thread = thread::Builder::new()
        .name("ihs-wl".into())
        .spawn(move || compositor_main(config, caps, ready_tx))
        .map_err(|e| Error::internal(format!("spawn compositor thread: {e}")))?;

    match ready_rx.recv() {
        Ok(Ok((tx, socket_name))) => {
            tracing::info!(socket = ?socket_name, "Wayland server listening");
            *guard = Some(Running {
                tx,
                thread,
                socket_name,
            });
            drop(guard);
            // Views can only be created once the factory is in; register it
            // last so a create never finds the thread missing.
            crate::view::register_factory();
            Ok(())
        }
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err(Error::internal("compositor thread exited during start-up"))
        }
    }
}

pub fn stop() -> Result<()> {
    let _lifecycle = LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner());
    if server().is_none() {
        return Err(Error::new(
            crate::IhsWlResult::ErrNotRunning as i32,
            "server not running",
        ));
    }
    crate::view::unregister_factory();
    let running = server().take().ok_or_else(|| {
        Error::new(
            crate::IhsWlResult::ErrNotRunning as i32,
            "server not running",
        )
    })?;
    let _ = running.tx.send(Cmd::Quit);
    running
        .thread
        .join()
        .map_err(|_| Error::internal("compositor thread panicked on shutdown"))?;
    tracing::info!("Wayland server stopped");
    Ok(())
}

pub fn send(cmd: Cmd) -> Result<()> {
    let guard = server();
    let running = guard.as_ref().ok_or_else(|| {
        Error::new(
            crate::IhsWlResult::ErrNotRunning as i32,
            "server not running",
        )
    })?;
    running.tx.send(cmd).map_err(|_| {
        Error::new(
            crate::IhsWlResult::ErrNotRunning as i32,
            "compositor thread has exited",
        )
    })
}

pub fn is_running() -> bool {
    server().as_ref().is_some_and(|r| !r.thread.is_finished())
}

/// Valid until `stop`.
pub fn socket_name_ptr() -> *const c_char {
    server()
        .as_ref()
        .map_or(std::ptr::null(), |r| r.socket_name.as_ptr())
}

pub fn socket_name() -> Option<String> {
    server()
        .as_ref()
        .map(|r| r.socket_name.to_string_lossy().into_owned())
}

fn compositor_main(config: Config, caps: crate::caps::Caps, ready: Ready) {
    let mut ready = Some(ready);
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| run(&config, &caps, &mut ready)));
    // By here the loop and the Display are dropped, so clients have already
    // seen the disconnect.
    let error = match outcome {
        Ok(Ok(())) => return,
        Ok(Err(e)) => e,
        Err(payload) => Error::internal(format!(
            "compositor thread panicked: {}",
            crate::panic_message(&*payload)
        )),
    };
    match ready.take() {
        Some(ready) => {
            let _ = ready.send(Err(error));
        }
        None => tracing::error!(%error, "compositor thread failed; clients disconnected"),
    }
}

fn run(config: &Config, caps: &crate::caps::Caps, ready: &mut Option<Ready>) -> Result<()> {
    let mut event_loop: EventLoop<'static, State> =
        EventLoop::try_new().map_err(|e| Error::internal(format!("calloop: {e}")))?;
    let display: Display<State> =
        Display::new().map_err(|e| Error::internal(format!("wl_display: {e}")))?;
    let dh = display.handle();

    let socket = bind_socket(config.socket_name.as_deref())?;
    let socket_name = CString::new(socket.socket_name().as_bytes()).unwrap();

    let handle = event_loop.handle();
    handle
        .insert_source(socket, |stream, _, state| state.accept_client(stream))
        .map_err(|e| Error::internal(format!("insert socket source: {}", e.error)))?;
    handle
        .insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                // SAFETY: the Display is never dropped from inside a dispatch.
                unsafe { display.get_mut().dispatch_clients(state)? };
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| Error::internal(format!("insert display source: {}", e.error)))?;

    let (tx, rx) = channel::channel::<Cmd>();
    handle
        .insert_source(rx, |event, _, state| match event {
            channel::Event::Msg(cmd) => state.handle_cmd(cmd),
            channel::Event::Closed => state.loop_signal.stop(),
        })
        .map_err(|e| Error::internal(format!("insert command channel: {}", e.error)))?;

    let mut state = State::new(dh, event_loop.get_signal(), handle.clone(), caps);

    if let Some(ready) = ready.take() {
        let _ = ready.send(Ok((tx, socket_name)));
    }

    event_loop
        .run(None, &mut state, |state| {
            state.reap_clients();
            if let Err(e) = state.dh.flush_clients() {
                tracing::warn!("flush_clients: {e}");
            }
        })
        .map_err(|e| Error::internal(format!("event loop: {e}")))
}

/// Bind the listening socket. Never looks at or sets this process's
/// `WAYLAND_DISPLAY` beyond choosing a prefix: on the wayland-* backends the
/// shell is itself a client of that socket.
fn bind_socket(name: Option<&str>) -> Result<ListeningSocketSource> {
    let bind_err = |name: &str, e: BindError| Error::io(format!("bind {name}: {e}"));
    if let Some(name) = name {
        return ListeningSocketSource::with_name(name).map_err(|e| bind_err(name, e));
    }
    let prefix = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "wayland-ihs-"
    } else {
        "wayland-"
    };
    for i in 0..32 {
        let name = format!("{prefix}{i}");
        match ListeningSocketSource::with_name(&name) {
            Ok(socket) => return Ok(socket),
            Err(BindError::AlreadyInUse) => continue,
            Err(e) => return Err(bind_err(&name, e)),
        }
    }
    Err(Error::io(format!("no free socket name {prefix}0..31")))
}
