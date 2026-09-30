// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

use smithay::delegate_xdg_shell;
use smithay::desktop::PopupKind;
use smithay::reexports::wayland_server::protocol::{wl_output, wl_seat};
use smithay::utils::Serial;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};

use crate::observe::{self, Observed};
use crate::state::{app_id_and_title, State, Toplevels};
use crate::{IhsWlCapability, IhsWlViewEvent};

impl State {
    /// The view showing toplevel @p surface, if one does and its app
    /// handles @p capability.
    fn view_handling(&self, surface: &ToplevelSurface, capability: IhsWlCapability) -> Option<i32> {
        let id = Toplevels::id_of(surface.wl_surface())?;
        let view_id = self.toplevels.by_id.get(&id)?.view?;
        let handled = self.views.get(&view_id)?.capabilities & capability as u32 != 0;
        handled.then_some(view_id)
    }

    /// Pass a window-state request of @p surface to the app showing it, if
    /// it handles that; xdg-shell has requests for capabilities not
    /// advertised ignored (a client older than wm_capabilities may send
    /// them).
    fn post_request(&self, surface: &ToplevelSurface, event: IhsWlViewEvent) {
        let capability = match event {
            IhsWlViewEvent::MaximizeRequested | IhsWlViewEvent::UnmaximizeRequested => {
                IhsWlCapability::Maximize
            }
            IhsWlViewEvent::MinimizeRequested => IhsWlCapability::Minimize,
            IhsWlViewEvent::FullscreenRequested | IhsWlViewEvent::UnfullscreenRequested => {
                IhsWlCapability::Fullscreen
            }
            IhsWlViewEvent::Bound | IhsWlViewEvent::Closed => return,
        };
        if let Some(view_id) = self.view_handling(surface, capability) {
            tracing::debug!(view_id, ?event, "window request");
            crate::events::post(event, view_id);
        }
    }

    /// Pass a request on and answer it: xdg-shell asks for a configure
    /// either way, and until the app grants the state (ihs_wl_view_state)
    /// it repeats what the toplevel has.
    fn window_request(&mut self, surface: &ToplevelSurface, event: IhsWlViewEvent) {
        self.post_request(surface, event);
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }
}

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
        // A dialog's view, looked up while its parent is still known.
        let dialog_view = surface
            .parent()
            .and_then(|_| self.bound_view_of(surface.wl_surface()));
        self.unbind_toplevel(id);
        if let Some(entry) = self.toplevels.by_id.remove(&id) {
            if entry.mapped {
                let (app_id, _) = app_id_and_title(surface.wl_surface());
                tracing::info!(id, app_id, "toplevel unmapped");
                observe::emit(Observed::ToplevelUnmapped { app_id });
            }
        }
        if let Some(view_id) = dialog_view {
            // Its layers go, and the keyboard goes back to what is below.
            self.submit_view(view_id);
            self.refresh_keyboard_focus();
        }
    }

    fn parent_changed(&mut self, surface: ToplevelSurface) {
        // Now a dialog of another view, or none: show it where it belongs.
        let views: Vec<i32> = self.views.keys().copied().collect();
        for view_id in views {
            if self
                .views
                .get(&view_id)
                .is_some_and(|v| v.toplevel.is_some())
            {
                self.submit_view(view_id);
            }
        }
        tracing::debug!(
            parent = surface.parent().is_some(),
            "toplevel parent changed"
        );
        self.refresh_keyboard_focus();
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.window_request(&surface, IhsWlViewEvent::MaximizeRequested);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.window_request(&surface, IhsWlViewEvent::UnmaximizeRequested);
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        // No configure: xdg-shell gives minimize none to wait for.
        self.post_request(&surface, IhsWlViewEvent::MinimizeRequested);
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<wl_output::WlOutput>,
    ) {
        self.window_request(&surface, IhsWlViewEvent::FullscreenRequested);
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.window_request(&surface, IhsWlViewEvent::UnfullscreenRequested);
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
