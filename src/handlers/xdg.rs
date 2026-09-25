// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

use smithay::delegate_xdg_shell;
use smithay::desktop::PopupKind;
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
        surface.with_pending_state(|state| state.positioner = positioner);
        self.constrain_popup(&surface);
        if let Err(e) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            tracing::warn!("track popup: {e:?}");
        }
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        // Its layers go with it.
        let view = surface
            .get_parent_surface()
            .and_then(|parent| self.bound_view_of(&parent));
        self.popups.cleanup();
        if let Some(view_id) = view {
            self.submit_view(view_id);
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| state.positioner = positioner);
        self.constrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn grab(&mut self, surface: PopupSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        self.grab_popup(surface, serial);
    }
}

delegate_xdg_shell!(State);
