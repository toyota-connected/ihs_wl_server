//! All smithay protocol state, owned by the compositor thread.

use std::collections::{BTreeMap, HashMap};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use smithay::input::{Seat, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::LoopSignal;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::compositor::{self, CompositorClientState, CompositorState};
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellState, XdgToplevelSurfaceData};
use smithay::wayland::shm::ShmState;

use crate::observe::{self, Observed};
use crate::thread::Cmd;
use crate::view::ViewHandle;

pub struct State {
    pub dh: DisplayHandle,
    pub loop_signal: LoopSignal,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    /// Input attaches its pointer/touch/keyboard capabilities here.
    #[allow(dead_code)]
    pub seat: Seat<Self>,
    /// Owns the xdg-output global.
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    /// Kept alive for the global's lifetime.
    #[allow(dead_code)]
    pub output: Output,

    pub toplevels: Toplevels,
    /// Live platform views, by Flutter view id.
    pub views: HashMap<i32, ViewHandle>,
    clients: Arc<AtomicU32>,
}

impl State {
    pub fn new(dh: DisplayHandle, loop_signal: LoopSignal) -> Self {
        let mut seat_state = SeatState::new();
        let seat = seat_state.new_wl_seat(&dh, "seat0");
        State {
            compositor_state: CompositorState::new::<Self>(&dh),
            xdg_shell_state: XdgShellState::new::<Self>(&dh),
            shm_state: ShmState::new::<Self>(&dh, vec![]),
            data_device_state: DataDeviceState::new::<Self>(&dh),
            seat_state,
            seat,
            output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(&dh),
            output: crate::handlers::output::virtual_output(&dh),
            dh,
            loop_signal,
            toplevels: Toplevels::default(),
            views: HashMap::new(),
            clients: Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn accept_client(&mut self, stream: UnixStream) {
        let data = Arc::new(ClientState {
            compositor_state: CompositorClientState::default(),
            clients: self.clients.clone(),
        });
        if let Err(e) = self.dh.insert_client(stream, data) {
            tracing::warn!("insert_client: {e}");
        }
    }

    pub fn handle_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Quit => self.loop_signal.stop(),
            Cmd::ViewCreated(view) => {
                if let Some(old) = self.views.insert(view.id, view) {
                    // A create under a live id disposes the incumbent first
                    // (hot restart), so this is only an ordering race.
                    tracing::debug!(id = old.id, "view replaced");
                }
            }
            Cmd::ViewDisposed(id) => {
                self.views.remove(&id);
            }
        }
    }
}

/// Per-client data. Counts connections for `ClientCount`.
pub struct ClientState {
    pub compositor_state: CompositorClientState,
    clients: Arc<AtomicU32>,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {
        let n = self.clients.fetch_add(1, Ordering::Relaxed) + 1;
        observe::emit(Observed::ClientCount(n));
    }

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {
        let n = self.clients.fetch_sub(1, Ordering::Relaxed) - 1;
        observe::emit(Observed::ClientCount(n));
    }
}

/// The toplevels the server knows, keyed by a server-assigned id.
///
/// A toplevel is *mapped* at its initial commit, once the client has had the
/// chance to set app_id and title -- which is what views bind by.
#[derive(Default)]
pub struct Toplevels {
    next_id: i64,
    pub by_id: BTreeMap<i64, ToplevelEntry>,
}

pub struct ToplevelEntry {
    pub surface: ToplevelSurface,
    pub mapped: bool,
}

/// The toplevel id, stored in the surface's data map.
struct ToplevelId(i64);

impl Toplevels {
    pub fn insert(&mut self, surface: ToplevelSurface) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        compositor::with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .insert_if_missing_threadsafe(|| ToplevelId(id));
        });
        self.by_id.insert(
            id,
            ToplevelEntry {
                surface,
                mapped: false,
            },
        );
        id
    }

    pub fn id_of(surface: &WlSurface) -> Option<i64> {
        compositor::with_states(surface, |states| {
            states.data_map.get::<ToplevelId>().map(|t| t.0)
        })
    }
}

pub fn app_id_and_title(surface: &WlSurface) -> (String, String) {
    compositor::with_states(surface, |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .map(|data| {
                let data = data.lock().unwrap();
                (
                    data.app_id.clone().unwrap_or_default(),
                    data.title.clone().unwrap_or_default(),
                )
            })
            .unwrap_or_default()
    })
}
