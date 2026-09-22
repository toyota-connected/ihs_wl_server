use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_surface::WlSurface};
use smithay::reexports::wayland_server::Client;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    self, CompositorClientState, CompositorHandler, CompositorState,
};
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

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if compositor::is_sync_subsurface(surface) {
            return;
        }
        self.toplevel_commit(surface);
        self.popup_commit(surface);
    }
}

impl State {
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
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(State);
delegate_shm!(State);
