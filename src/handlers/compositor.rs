use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::reexports::calloop::Interest;
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    self, BufferAssignment, CompositorClientState, CompositorHandler, CompositorState,
    SurfaceAttributes,
};
use smithay::wayland::dmabuf::get_dmabuf;
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
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if compositor::is_sync_subsurface(surface) {
            // Applied with its parent's commit, which submits the tree.
            return;
        }
        self.toplevel_commit(surface);
        self.popup_commit(surface);
        if let Some(view_id) = self.bound_view_of(surface) {
            self.submit_view(view_id);
        }
    }
}

impl State {
    /// Hold a commit that attaches a dma-buf until the client's rendering
    /// into it is done, so the shell is only ever handed finished buffers and
    /// needs no acquire fence. The buffer's implicit fence decides: its fds
    /// poll readable once every write to it has completed.
    fn gate_on_readiness(&mut self, surface: &WlSurface) {
        let dmabuf = compositor::with_states(surface, |states| {
            let mut attrs = states.cached_state.get::<SurfaceAttributes>();
            match attrs.pending().buffer.as_ref() {
                Some(BufferAssignment::NewBuffer(buffer)) => get_dmabuf(buffer).ok().cloned(),
                _ => None,
            }
        });
        let Some(dmabuf) = dmabuf else {
            return;
        };
        // Already done: nothing to wait for.
        let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) else {
            return;
        };
        let Some(client) = surface.client() else {
            return;
        };
        let inserted = self.loop_handle.insert_source(source, move |_, _, state| {
            let dh = state.dh.clone();
            state
                .client_compositor_state(&client)
                .blocker_cleared(state, &dh);
            Ok(())
        });
        match inserted {
            Ok(_) => compositor::add_blocker(surface, blocker),
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
