// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! All smithay protocol state, owned by the compositor thread.

use std::collections::{BTreeMap, HashMap};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use smithay::desktop::PopupManager;
use smithay::input::{Seat, SeatState};
use smithay::output::{Output, Scale};
use smithay::reexports::calloop::{LoopHandle, LoopSignal, RegistrationToken};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Weak};
use smithay::wayland::commit_timing::CommitTimingManagerState;
use smithay::wayland::compositor::{self, CompositorClientState, CompositorState};
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufState};
use smithay::wayland::drm_syncobj::DrmSyncobjState;
use smithay::wayland::fifo::FifoManagerState;
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellState, XdgToplevelSurfaceData};
use smithay::wayland::shm::ShmState;
use smithay::wayland::viewporter::ViewporterState;

use crate::buffers::BufferIds;
use crate::caps::Caps;
use crate::observe::{self, Observed};
use crate::pacing::Loose;
use crate::submit::Holds;
use crate::thread::Cmd;
use crate::timing::{FrameClock, Frames};
use crate::view::ViewHandle;

pub struct State {
    pub dh: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub loop_handle: LoopHandle<'static, State>,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    /// Kept alive for the global's lifetime.
    #[allow(dead_code)]
    pub dmabuf_global: DmabufGlobal,
    #[allow(dead_code)]
    pub viewporter_state: ViewporterState,
    #[allow(dead_code)]
    pub presentation_state: PresentationState,
    #[allow(dead_code)]
    pub fifo_manager_state: FifoManagerState,
    #[allow(dead_code)]
    pub fractional_scale_state: FractionalScaleManagerState,
    #[allow(dead_code)]
    pub commit_timing_manager_state: CommitTimingManagerState,
    /// Explicit sync, when a render node can wait on syncobjs.
    pub syncobj_state: Option<DrmSyncobjState>,
    /// Buffers an EGL implementation shares over its own protocol.
    pub egl: crate::egl_display::EglBuffers,
    /// gbm_buffer_backend, when the loaded libgbm can import its buffers.
    pub gbm_buffer: Option<crate::gbm_buffer::GbmBufferState>,
    /// What the shell imports.
    pub caps: Caps,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    /// Kept alive for the global's lifetime.
    #[allow(dead_code)]
    pub seat: Seat<Self>,
    pub devices: crate::input::Devices,
    /// The view Dart last gave keyboard focus to.
    pub focused_view: Option<i32>,
    /// Owns the xdg-output global.
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    /// Kept alive for the global's lifetime.
    #[allow(dead_code)]
    pub output: Output,

    pub toplevels: Toplevels,
    pub popups: PopupManager,
    /// Live platform views, by Flutter view id.
    pub views: HashMap<i32, ViewEntry>,
    pub buffers: BufferIds,
    /// Stages shared-memory surfaces into dma-bufs.
    pub stager: crate::staging::Stager,
    clients: Arc<AtomicU32>,

    /// The display's refresh cycle, from the shell's last report.
    pub clock: FrameClock,
    /// Surfaces with updates waiting for their target time.
    pub timed: Vec<Weak<WlSurface>>,
    /// Fifo barriers of updates no view shows.
    pub loose: Vec<Loose>,
    /// The clock's next tick, while anything waits on it.
    pub tick: Option<(RegistrationToken, u64)>,

    /// Commits waiting for their buffers to be ready: the event sources
    /// that release them, by client.
    pub waits: HashMap<ClientId, HashMap<u64, RegistrationToken>>,
    pub next_wait: u64,
    /// Clients disconnected since the last look, as their data records it.
    gone: Arc<Mutex<Vec<ClientId>>>,
}

/// A live platform view and what it shows.
pub struct ViewEntry {
    pub handle: ViewHandle,
    /// The toplevel it shows, once bound.
    pub toplevel: Option<i64>,
    /// Its laid-out size in physical pixels, once known.
    pub size: Option<(i32, i32)>,
    /// Physical pixels per logical one.
    pub dpr: f64,
    /// The seq of its last submit.
    pub seq: u64,
    /// Buffers the shell may still be using.
    pub holds: Holds,
    /// Its last submit showed nothing.
    pub cleared: bool,
    /// Submitted frames awaiting the shell's report.
    pub frames: Frames,
    /// Out of the scene: nothing it shows is reported.
    pub suspended: bool,
}

impl State {
    pub fn new(
        dh: DisplayHandle,
        loop_signal: LoopSignal,
        loop_handle: LoopHandle<'static, State>,
        caps: &Caps,
    ) -> Self {
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "seat0");
        let devices = crate::input::Devices::add(&mut seat);
        let mut dmabuf_state = DmabufState::new();
        let dmabuf_global = crate::handlers::dmabuf::global(&mut dmabuf_state, &dh, caps);
        State {
            // v6: preferred_buffer_scale, the integer fallback for scale.
            compositor_state: CompositorState::new_v6::<Self>(&dh),
            xdg_shell_state: XdgShellState::new::<Self>(&dh),
            shm_state: ShmState::new::<Self>(&dh, vec![]),
            dmabuf_state,
            dmabuf_global,
            viewporter_state: ViewporterState::new::<Self>(&dh),
            // The shell reports CLOCK_MONOTONIC times.
            presentation_state: PresentationState::new::<Self>(&dh, libc::CLOCK_MONOTONIC as u32),
            fifo_manager_state: FifoManagerState::new::<Self>(&dh),
            fractional_scale_state: FractionalScaleManagerState::new::<Self>(&dh),
            commit_timing_manager_state: CommitTimingManagerState::new::<Self>(&dh),
            syncobj_state: crate::syncobj::device()
                .map(|device| DrmSyncobjState::new::<Self>(&dh, device)),
            egl: crate::egl_display::EglBuffers::bind(&dh, caps),
            gbm_buffer: crate::gbm_buffer::GbmBufferState::new(&dh),
            caps: caps.clone(),
            data_device_state: DataDeviceState::new::<Self>(&dh),
            seat_state,
            seat,
            devices,
            focused_view: None,
            output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(&dh),
            output: crate::handlers::output::virtual_output(&dh),
            dh,
            loop_signal,
            loop_handle,
            toplevels: Toplevels::default(),
            popups: PopupManager::default(),
            views: HashMap::new(),
            buffers: BufferIds::default(),
            stager: crate::staging::Stager::default(),
            clients: Arc::new(AtomicU32::new(0)),
            clock: FrameClock::default(),
            timed: Vec::new(),
            loose: Vec::new(),
            tick: None,
            waits: HashMap::new(),
            next_wait: 0,
            gone: Arc::default(),
        }
    }

    pub fn accept_client(&mut self, stream: UnixStream) {
        let data = Arc::new(ClientState {
            compositor_state: CompositorClientState::default(),
            clients: self.clients.clone(),
            gone: self.gone.clone(),
        });
        match self.dh.insert_client(stream, data) {
            // Counted here: libwayland-server never calls `initialized`.
            Ok(_) => {
                let n = self.clients.fetch_add(1, Ordering::Relaxed) + 1;
                observe::emit(Observed::ClientCount(n));
            }
            Err(e) => tracing::warn!("insert_client: {e}"),
        }
    }

    /// Drop what disconnected clients left waiting: nothing will apply
    /// their commits now, and a wait on a point that is never signaled
    /// would hold its fd for good.
    pub fn reap_clients(&mut self) {
        let gone = std::mem::take(&mut *self.gone.lock().unwrap_or_else(|e| e.into_inner()));
        for id in gone {
            let waits = self.waits.remove(&id).unwrap_or_default();
            if waits.is_empty() {
                continue;
            }
            observe::emit(Observed::WaitsDropped { count: waits.len() });
            for (_, token) in waits {
                self.loop_handle.remove(token);
            }
        }
    }

    pub fn handle_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Quit => self.loop_signal.stop(),
            Cmd::ViewCreated(view) => {
                let id = view.id;
                let size = view.size;
                let dpr = view.params.dpr.unwrap_or(1.0);
                // One output stands for the display every view is on.
                self.output
                    .change_current_state(None, None, Some(Scale::Fractional(dpr)), None);
                if self.views.contains_key(&id) {
                    // A create under a live id disposes the incumbent first
                    // (hot restart), so this is only an ordering race.
                    tracing::debug!(id, "view replaced");
                    self.unbind_view(id);
                }
                self.views.insert(
                    id,
                    ViewEntry {
                        handle: view,
                        toplevel: None,
                        size,
                        dpr,
                        seq: 0,
                        holds: Holds::default(),
                        cleared: false,
                        frames: Frames::default(),
                        suspended: false,
                    },
                );
                self.try_bind_view(id);
            }
            Cmd::ViewDisposed(id) => {
                self.unbind_view(id);
                self.views.remove(&id);
                self.buffers.forget_view(id);
                if self.focused_view == Some(id) {
                    self.focused_view = None;
                }
            }
            Cmd::ViewResized {
                view_id,
                width,
                height,
            } => {
                if let Some(entry) = self.views.get_mut(&view_id) {
                    entry.size = Some((width, height));
                }
                self.configure_view(view_id);
            }
            Cmd::Presented { view_id, report } => self.frame_presented(view_id, report),
            Cmd::ViewSuspended { view_id, suspended } => {
                self.view_suspended(view_id, suspended);
                self.refresh_keyboard_focus();
            }
            Cmd::Pointer { view_id, event } => self.pointer_input(view_id, event),
            Cmd::Touch { view_id, event } => self.touch_input(view_id, event),
            Cmd::Key {
                view_id,
                evdev,
                pressed,
                time_us,
            } => self.key_input(view_id, evdev, pressed, time_us),
            Cmd::Focus { view_id, focused } => self.focus_input(view_id, focused),
        }
    }

    /// Bind @p view_id to a toplevel if it has none: the oldest mapped,
    /// unbound one with the view's app_id, or with any app_id when the view
    /// names neither an app_id nor a token. Binding by activation token is
    /// not wired up yet, so a view that names only a token waits.
    pub fn try_bind_view(&mut self, view_id: i32) {
        let Some(entry) = self.views.get(&view_id) else {
            return;
        };
        if entry.toplevel.is_some() {
            return;
        }
        let params = &entry.handle.params;
        if params.app_id.is_none() && params.token.is_some() {
            return;
        }
        let want = params.app_id.clone();
        let found = self.toplevels.by_id.iter().find_map(|(id, t)| {
            if !t.mapped || t.view.is_some() {
                return None;
            }
            match &want {
                Some(app_id) => {
                    (app_id_and_title(t.surface.wl_surface()).0 == *app_id).then_some(*id)
                }
                None => Some(*id),
            }
        });
        let Some(toplevel_id) = found else {
            return;
        };
        self.toplevels.by_id.get_mut(&toplevel_id).unwrap().view = Some(view_id);
        self.views.get_mut(&view_id).unwrap().toplevel = Some(toplevel_id);
        tracing::info!(view_id, toplevel_id, "view bound");
        observe::emit(Observed::ViewBound {
            view_id,
            toplevel_id,
        });
        // Known before the first configure, so the client sizes its first
        // buffer for it.
        self.scale_tree(view_id);
        self.configure_view(view_id);
        self.refresh_keyboard_focus();
        // Show what the toplevel already has; a static client may never
        // commit again.
        self.submit_view(view_id);
    }

    /// A toplevel mapped: views waiting for one may bind it, oldest first.
    pub fn bind_waiting_views(&mut self) {
        let mut waiting: Vec<i32> = self
            .views
            .iter()
            .filter(|(_, v)| v.toplevel.is_none())
            .map(|(id, _)| *id)
            .collect();
        waiting.sort_unstable();
        for id in waiting {
            self.try_bind_view(id);
        }
    }

    /// Break the view's binding, handing back every buffer it held.
    pub fn unbind_view(&mut self, view_id: i32) {
        self.drop_frames(view_id);
        // Else the shell keeps showing the toplevel's last frame.
        self.clear_view(view_id);
        let Some(entry) = self.views.get_mut(&view_id) else {
            return;
        };
        entry.holds.clear(&self.loop_handle);
        if let Some(toplevel_id) = entry.toplevel.take() {
            if let Some(t) = self.toplevels.by_id.get_mut(&toplevel_id) {
                t.view = None;
            }
            tracing::info!(view_id, toplevel_id, "view unbound");
        }
        self.refresh_keyboard_focus();
    }

    /// The toplevel is going away: its view shows nothing new until it binds
    /// another.
    pub fn unbind_toplevel(&mut self, toplevel_id: i64) {
        let view = self.toplevels.by_id.get(&toplevel_id).and_then(|t| t.view);
        if let Some(view_id) = view {
            self.unbind_view(view_id);
        }
    }

    /// Size the view's toplevel to the view, in logical pixels.
    pub fn configure_view(&mut self, view_id: i32) {
        let Some(entry) = self.views.get(&view_id) else {
            return;
        };
        let (Some(toplevel_id), Some((w, h))) = (entry.toplevel, entry.size) else {
            return;
        };
        let (w, h) = (
            (w as f64 / entry.dpr).round() as i32,
            (h as f64 / entry.dpr).round() as i32,
        );
        let Some(t) = self.toplevels.by_id.get(&toplevel_id) else {
            return;
        };
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
        let changed = t.surface.with_pending_state(|s| {
            let size = Some((w.max(1), h.max(1)).into());
            let before = (s.size, s.states.clone());
            s.size = size;
            // A tile the size of its view: no borders to draw, no resizing.
            s.states.set(xdg_toplevel::State::Maximized);
            s.states.set(xdg_toplevel::State::Activated);
            before != (s.size, s.states.clone())
        });
        if changed && t.surface.is_initial_configure_sent() {
            t.surface.send_configure();
        }
    }

    /// The view showing the toplevel @p surface belongs to, popups
    /// included, if it is bound.
    pub fn bound_view_of(&self, surface: &WlSurface) -> Option<i32> {
        let root = crate::popups::toplevel_of(&self.popups, surface);
        let id = Toplevels::id_of(&root)?;
        self.toplevels.by_id.get(&id)?.view
    }
}

/// Per-client data. Counts connections for `ClientCount`.
pub struct ClientState {
    pub compositor_state: CompositorClientState,
    clients: Arc<AtomicU32>,
    gone: Arc<Mutex<Vec<ClientId>>>,
}

impl ClientData for ClientState {
    fn disconnected(&self, client_id: ClientId, _reason: DisconnectReason) {
        self.gone
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(client_id);
        // Never below zero: a panic here, called from libwayland-server,
        // would abort the shell.
        let n = self
            .clients
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                Some(n.saturating_sub(1))
            })
            .unwrap_or(0)
            .saturating_sub(1);
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
    /// The view showing it, once bound.
    pub view: Option<i32>,
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
                view: None,
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
