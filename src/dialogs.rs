// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Dialogs: toplevels with a parent (`xdg_toplevel.set_parent` -- About,
//! file choosers, message boxes), shown in the view of their top-level
//! ancestor.
//!
//! A view's windows are its toplevel, then its dialogs, oldest first, each
//! centered over its parent and kept inside the view; each window's popups
//! stack above it. A dialog never binds a view of its own, the topmost
//! window has the keyboard and is the activated one, and input reaches
//! whatever is on top -- a modal dialog's client ignores its parent itself.

use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};

use crate::state::{State, Toplevels};

/// Parents followed at most: a loop in a client's parents ends here.
const MAX_DEPTH: usize = 16;

/// A window of a view's scene: its toplevel surface, and where its window
/// geometry's origin lands in the view (client logical pixels).
pub type Window = (WlSurface, Point<i32, Logical>);

/// The size of @p surface's window: its geometry, or what its tree covers
/// before it set one.
fn window_size(surface: &WlSurface) -> (i32, i32) {
    crate::tree::geometry_size(surface).unwrap_or_else(|| {
        let bbox = bbox_from_surface_tree(surface, (0, 0));
        (bbox.size.w, bbox.size.h)
    })
}

impl State {
    /// The toplevel surface @p surface is shown with: through subsurfaces
    /// and popups to its toplevel, then up its dialogs' parents.
    pub fn window_root(&self, surface: &WlSurface) -> WlSurface {
        let mut root = crate::popups::toplevel_of(&self.popups, surface);
        for _ in 0..MAX_DEPTH {
            let parent = Toplevels::id_of(&root)
                .and_then(|id| self.toplevels.by_id.get(&id))
                .and_then(|t| t.surface.parent());
            let Some(parent) = parent else {
                break;
            };
            root = crate::popups::toplevel_of(&self.popups, &parent);
        }
        root
    }

    /// The windows view @p view_id shows, bottom to top: its toplevel at the
    /// view's origin, then each mapped dialog over it, oldest first,
    /// centered over its parent and inside the view.
    pub fn view_windows(&self, view_id: i32) -> Vec<Window> {
        let Some(root) = self.view_root(view_id) else {
            return Vec::new();
        };
        let bounds = self
            .views
            .get(&view_id)
            .and_then(|v| v.client_size())
            .unwrap_or_else(|| window_size(&root));
        let mut windows: Vec<Window> = vec![(root.clone(), (0, 0).into())];
        // Ids grow with creation, so a parent comes before its dialogs.
        for t in self.toplevels.by_id.values() {
            let surface = t.surface.wl_surface();
            let Some(parent) = t.surface.parent() else {
                continue;
            };
            if !t.mapped || self.window_root(surface) != root {
                continue;
            }
            let parent = crate::popups::toplevel_of(&self.popups, &parent);
            let Some(&(_, at)) = windows.iter().find(|(w, _)| *w == parent) else {
                continue;
            };
            let (pw, ph) = window_size(&parent);
            let (w, h) = window_size(surface);
            let x = (at.x + (pw - w) / 2).clamp(0, (bounds.0 - w).max(0));
            let y = (at.y + (ph - h) / 2).clamp(0, (bounds.1 - h).max(0));
            windows.push((surface.clone(), (x, y).into()));
        }
        windows
    }

    /// The window of view @p view_id that has the keyboard: its topmost.
    pub fn focus_window(&self, view_id: i32) -> Option<WlSurface> {
        self.view_windows(view_id).pop().map(|(w, _)| w)
    }
}
