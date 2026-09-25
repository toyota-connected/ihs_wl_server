// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Scale: a client renders once, at the size its view has on screen.
//!
//! A view's toplevel is configured in logical pixels (its physical size over
//! the view's device pixel ratio), and every surface of its tree is told the
//! ratio: exactly through wp_fractional_scale_v1, rounded up through
//! wl_surface.preferred_buffer_scale, and through the output it enters. A
//! client that follows it attaches buffers the view's physical size, which
//! the shell shows 1:1.

use smithay::delegate_fractional_scale;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Transform;
use smithay::wayland::compositor::{self, TraversalAction};
use smithay::wayland::fractional_scale::{with_fractional_scale, FractionalScaleHandler};

use crate::state::State;

impl FractionalScaleHandler for State {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        if let Some(view_id) = self.bound_view_of(&surface) {
            self.scale_tree(view_id);
        }
    }
}

delegate_fractional_scale!(State);

impl State {
    /// Tell every surface of the scene view @p view_id shows its scale.
    /// Each signal is only sent when it changes.
    pub fn scale_tree(&mut self, view_id: i32) {
        let Some(dpr) = self.views.get(&view_id).map(|v| v.dpr) else {
            return;
        };
        let output = &self.output;
        for (root, _) in self.view_trees(view_id) {
            compositor::with_surface_tree_downward(
                &root,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |surface, states, _| {
                    with_fractional_scale(states, |fs| fs.set_preferred_scale(dpr));
                    compositor::send_surface_state(
                        surface,
                        states,
                        dpr.ceil() as i32,
                        Transform::Normal,
                    );
                    output.enter(surface);
                },
                |_, _, _| true,
            );
        }
    }
}
