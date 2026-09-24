use smithay::delegate_xdg_shell;
use smithay::reexports::wayland_server::protocol::wl_seat;
use smithay::utils::Serial;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::observe::{self, Observed};
use crate::state::{app_id_and_title, State, Toplevels};

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let id = self.toplevels.insert(surface);
        tracing::debug!(id, "new toplevel");
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let Some(id) = Toplevels::id_of(surface.wl_surface()) else {
            return;
        };
        self.unbind_toplevel(id);
        if let Some(entry) = self.toplevels.by_id.remove(&id) {
            if entry.mapped {
                let (app_id, _) = app_id_and_title(surface.wl_surface());
                tracing::info!(id, app_id, "toplevel unmapped");
                observe::emit(Observed::ToplevelUnmapped { app_id });
            }
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        // Not yet constrained to the toplevel's bounds.
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
        });
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // Popup grabs need seat input, which is not wired up yet.
    }
}

delegate_xdg_shell!(State);
