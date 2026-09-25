// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Popups: menus, tooltips and the like, shown in their toplevel's view.
//!
//! A view shows a scene: its toplevel's surface tree, then each popup's
//! tree above it, parents below their children. Popups are placed inside
//! the view -- they cannot spill out of it -- and go away with a press
//! outside the client, when the view loses keyboard focus, or when it
//! leaves the scene.

use smithay::desktop::{
    find_popup_root_surface, get_popup_toplevel_coords, PopupKeyboardGrab, PopupKind, PopupManager,
    PopupPointerGrab, PopupUngrabStrategy,
};
use smithay::input::pointer::Focus;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Serial};
use smithay::wayland::compositor;
use smithay::wayland::shell::xdg::{PopupSurface, XDG_POPUP_ROLE};

use crate::state::State;

/// A surface tree of a view's scene, and where its root surface's origin
/// lands in the view, in logical pixels.
pub type Tree = (WlSurface, Point<i32, Logical>);

/// The toplevel @p surface belongs to: through its subsurface parents, then
/// its popup parents.
pub fn toplevel_of(popups: &PopupManager, surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = compositor::get_parent(&root) {
        root = parent;
    }
    if compositor::get_role(&root) == Some(XDG_POPUP_ROLE) {
        if let Some(toplevel) = popups
            .find_popup(&root)
            .and_then(|p| find_popup_root_surface(&p).ok())
        {
            return toplevel;
        }
    }
    root
}

impl State {
    /// The scene of view @p view_id, bottom to top; empty while unbound.
    pub fn view_trees(&self, view_id: i32) -> Vec<Tree> {
        let Some(root) = self.view_root(view_id) else {
            return Vec::new();
        };
        // The toplevel's window geometry starts at the view's origin.
        let origin = crate::tree::geometry_origin(&root);
        let mut trees = vec![(root.clone(), (-origin.x, -origin.y).into())];
        // Children come before their parents here.
        let mut popups: Vec<Tree> = PopupManager::popups_for_surface(&root)
            .map(|(popup, at)| (popup.wl_surface().clone(), at - popup.geometry().loc))
            .collect();
        popups.reverse();
        trees.extend(popups);
        trees
    }

    /// The toplevel surface view @p view_id shows.
    pub fn view_root(&self, view_id: i32) -> Option<WlSurface> {
        let toplevel = self.views.get(&view_id)?.toplevel?;
        Some(
            self.toplevels
                .by_id
                .get(&toplevel)?
                .surface
                .wl_surface()
                .clone(),
        )
    }

    /// The view's logical size, once laid out.
    fn view_logical_size(&self, view_id: i32) -> Option<(i32, i32)> {
        let entry = self.views.get(&view_id)?;
        let (w, h) = entry.size?;
        Some((
            (w as f64 / entry.dpr).round() as i32,
            (h as f64 / entry.dpr).round() as i32,
        ))
    }

    /// Place @p popup where its positioner asks, moved or flipped as it
    /// allows to stay inside its view.
    pub fn constrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        // In the parent's window geometry space, as the positioner is.
        // Worked out first: it reads the popup's state, which the closure
        // below holds locked.
        let target = self
            .bound_view_of(&root)
            .and_then(|v| self.view_logical_size(v))
            .map(|(w, h)| {
                Rectangle::new(
                    Point::from((0, 0)) - get_popup_toplevel_coords(&kind),
                    (w, h).into(),
                )
            });
        popup.with_pending_state(|state| {
            state.geometry = match target {
                Some(target) => state.positioner.get_unconstrained_geometry(target),
                None => state.positioner.get_geometry(),
            };
        });
    }

    /// Give @p popup the explicit grab its client asked for with @p serial:
    /// input goes to its client until a press lands outside it.
    pub fn grab_popup(&mut self, popup: PopupSurface, serial: Serial) {
        let kind = PopupKind::Xdg(popup);
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let seat = self.seat.clone();
        let Ok(mut grab) = self.popups.grab_popup(root, kind, &seat, serial) else {
            return;
        };
        // Only while the press that asked for it holds.
        let pointer = self.devices.pointer.clone();
        let previous = grab.previous_serial().unwrap_or_else(|| grab.serial());
        if pointer.is_grabbed() && !(pointer.has_grab(serial) || pointer.has_grab(previous)) {
            grab.ungrab(PopupUngrabStrategy::All);
            return;
        }
        if let Some(keyboard) = self.devices.keyboard.clone() {
            if keyboard.is_grabbed() && !(keyboard.has_grab(serial) || keyboard.has_grab(previous))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
    }

    /// Dismiss every popup of view @p view_id.
    pub fn dismiss_popups(&mut self, view_id: i32) {
        let Some(root) = self.view_root(view_id) else {
            return;
        };
        let popups: Vec<PopupKind> = PopupManager::popups_for_surface(&root)
            .map(|(p, _)| p)
            .collect();
        if popups.is_empty() {
            return;
        }
        for popup in &popups {
            let _ = PopupManager::dismiss_popup(&root, popup);
        }
        self.popups.cleanup();
    }
}
