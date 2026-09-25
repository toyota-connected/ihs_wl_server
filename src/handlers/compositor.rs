// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

use smithay::backend::renderer::sync::Fence;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::reexports::calloop::{EventSource, Interest};
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    self, Blocker, BufferAssignment, CompositorClientState, CompositorHandler, CompositorState,
    SurfaceAttributes,
};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::drm_syncobj::DrmSyncobjCachedState;
use smithay::wayland::shell::xdg::XDG_POPUP_ROLE;
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{delegate_compositor, delegate_shm};

use crate::observe::{self, Observed};
use crate::state::{app_id_and_title, ClientState, State, Toplevels};

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        compositor::add_pre_commit_hook::<Self, _>(surface, |state, _dh, surface| {
            state.gate_on_readiness(surface);
            state.watch_target(surface);
        });
        // Its staging slots go with it; the views drop their imports.
        compositor::add_destruction_hook::<Self, _>(surface, |state, surface| {
            let retired = compositor::with_states(surface, crate::staging::drop_ring);
            for uid in retired {
                state.retire_key(&crate::buffers::BufferKey::Staged(uid));
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        crate::timing::bump_generation(surface);
        if compositor::is_sync_subsurface(surface) {
            // Applied with its parent's commit, which submits the tree.
            return;
        }
        self.popups.commit(surface);
        self.toplevel_commit(surface);
        self.popup_commit(surface);
        match self.bound_view_of(surface) {
            Some(view_id) => self.submit_view(view_id),
            None => {
                let mut root = surface.clone();
                while let Some(parent) = compositor::get_parent(&root) {
                    root = parent;
                }
                self.not_shown(&root);
            }
        }
    }
}

/// Commits of one client waiting for their buffers at once, at most.
const MAX_WAITS: usize = 32;

impl State {
    /// Hold a commit that attaches a dma-buf until the client's rendering
    /// into it is done, so the shell is only ever handed finished buffers and
    /// needs no acquire fence. With explicit sync the commit's acquire point
    /// decides; otherwise the buffer's implicit fence does: its fds poll
    /// readable once every write to it has completed.
    fn gate_on_readiness(&mut self, surface: &WlSurface) {
        let (dmabuf, acquire) = compositor::with_states(surface, |states| {
            let mut attrs = states.cached_state.get::<SurfaceAttributes>();
            let dmabuf = match attrs.pending().buffer.as_ref() {
                Some(BufferAssignment::NewBuffer(buffer)) => get_dmabuf(buffer).ok().cloned(),
                _ => None,
            };
            let mut sync = states.cached_state.get::<DrmSyncobjCachedState>();
            (dmabuf, sync.pending().acquire_point.clone())
        });
        // An acquire point without a dma-buf is a protocol error, which the
        // syncobj surface's own hook posts.
        let Some(dmabuf) = dmabuf else {
            return;
        };
        let Some(client) = surface.client() else {
            return;
        };
        match acquire {
            // Explicit sync replaces implicit: the buffer may carry no
            // implicit fence at all.
            Some(acquire) => {
                if acquire.is_signaled() {
                    return;
                }
                match acquire.generate_blocker() {
                    Ok((blocker, source)) => self.hold_commit(surface, client, blocker, source),
                    // Cannot wait for it: let the commit through rather than
                    // wedge the client.
                    Err(e) => tracing::warn!("acquire point eventfd: {e}"),
                }
            }
            None => {
                // Already done: nothing to wait for.
                if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) {
                    self.hold_commit(surface, client, blocker, source);
                }
            }
        }
    }

    /// Hold @p surface's commit on @p blocker until @p source fires.
    ///
    /// Each wait holds an fd in the shell's process until it fires, and an
    /// explicit-sync point may never be signaled. So a client gets at most
    /// MAX_WAITS at a time -- a commit past that still queues behind the
    /// earlier ones, only without a wait of its own -- and loses them when it
    /// disconnects.
    fn hold_commit<S>(
        &mut self,
        surface: &WlSurface,
        client: Client,
        blocker: impl Blocker + Send + 'static,
        source: S,
    ) where
        S: EventSource<Event = (), Ret = std::io::Result<()>> + 'static,
    {
        let id = client.id();
        let waits = self.waits.entry(id.clone()).or_default();
        if waits.len() >= MAX_WAITS {
            tracing::debug!(?id, "too many commits waiting; not waiting on another");
            return;
        }
        self.next_wait += 1;
        let key = self.next_wait;
        let inserted = self.loop_handle.insert_source(source, move |_, _, state| {
            if let Some(waits) = state.waits.get_mut(&client.id()) {
                waits.remove(&key);
            }
            let dh = state.dh.clone();
            state
                .client_compositor_state(&client)
                .blocker_cleared(state, &dh);
            Ok(())
        });
        match inserted {
            Ok(token) => {
                self.waits.entry(id).or_default().insert(key, token);
                compositor::add_blocker(surface, blocker);
            }
            // Without a source nothing would ever clear the blocker; let the
            // commit through rather than wedge the client.
            Err(e) => tracing::warn!("readiness source: {e}"),
        }
    }

    fn toplevel_commit(&mut self, surface: &WlSurface) {
        let Some(id) = Toplevels::id_of(surface) else {
            return;
        };
        let Some(entry) = self.toplevels.by_id.get_mut(&id) else {
            return;
        };
        if !entry.surface.is_initial_configure_sent() {
            // Size 0x0: the client picks until a view is bound.
            entry.surface.send_configure();
        }
        if !entry.mapped {
            entry.mapped = true;
            let (app_id, title) = app_id_and_title(surface);
            tracing::info!(id, app_id, title, "toplevel mapped");
            observe::emit(Observed::ToplevelMapped { app_id, title });
            self.bind_waiting_views();
        }
    }

    fn popup_commit(&mut self, surface: &WlSurface) {
        if compositor::get_role(surface) != Some(XDG_POPUP_ROLE) {
            return;
        }
        let popup = self
            .xdg_shell_state
            .popup_surfaces()
            .iter()
            .find(|p| p.wl_surface() == surface)
            .cloned();
        if let Some(popup) = popup {
            if !popup.is_initial_configure_sent() {
                if let Err(e) = popup.send_configure() {
                    tracing::warn!("popup initial configure: {e}");
                }
            }
        }
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, buffer: &wl_buffer::WlBuffer) {
        self.retire_buffer(buffer);
    }
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(State);
delegate_shm!(State);
