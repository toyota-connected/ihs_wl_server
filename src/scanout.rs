// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Per-surface dma-buf feedback: a scanout tranche for a surface whose layer
//! could go on a KMS plane but for its buffer's format.
//!
//! The shell tells a view when one of its layers could bypass composition
//! if its buffer were in another format (IhsPvCallbacks::scanout_hint): the
//! formats the planes scan out, and the KMS device they belong to. The
//! surface behind that layer then gets feedback with a scanout tranche for
//! that device ahead of the main one, so a client allocates its next buffers
//! for a plane. The tranche offers only what the shell also imports, since
//! a hint is no promise of a plane and the shell may composite the buffer
//! after all.
//!
//! The shell withdraws a hint once the layer's format fits -- which is the
//! tranche working, so the tranche stays: taking it away would have the
//! client allocate for the GPU again, and the shell hint again. The surface
//! goes back to the default feedback when it leaves its view.

use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Resource, Weak};
use smithay::wayland::compositor::{self, TraversalAction};
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufFeedbackBuilder, SurfaceDmabufFeedbackState};

use crate::caps::FormatModifier;
use crate::handlers::dmabuf::formats;
use crate::state::State;

/// What the shell hinted for one layer of a view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub layer_id: u32,
    /// The KMS device's dev_t.
    pub dev: u64,
    /// What its planes scan out; empty withdraws the hint.
    pub formats: Vec<FormatModifier>,
}

/// The surfaces with a scanout tranche, and feedback built for a hint.
/// Kept apart from the surfaces' own state: smithay asks for a new surface
/// feedback with that state locked.
#[derive(Default)]
pub struct Scanout {
    /// Each with its view.
    hinted: Vec<(Weak<WlSurface>, i32, DmabufFeedback)>,
    /// Kept while hints keep asking for the same.
    last: Option<Built>,
}

struct Built {
    dev: u64,
    hinted: Vec<FormatModifier>,
    /// None when the shell imports none of the hinted formats.
    feedback: Option<DmabufFeedback>,
}

impl State {
    /// The feedback a surface with a scanout tranche gets when it asks for
    /// feedback after the hint came.
    pub fn hinted_feedback(&self, surface: &WlSurface) -> Option<DmabufFeedback> {
        self.scanout
            .hinted
            .iter()
            .find(|(s, _, _)| s.upgrade().is_ok_and(|s| s == *surface))
            .map(|(_, _, f)| f.clone())
    }

    /// The shell's hint for layer @p hint.layer_id of view @p view_id.
    pub fn scanout_hint(&mut self, view_id: i32, hint: Hint) {
        if hint.formats.is_empty() {
            tracing::debug!(view_id, layer_id = hint.layer_id, "scanout hint withdrawn");
            return;
        }
        // Without a main device there is no feedback to add a tranche to.
        let (Some(main), Some(default)) = (self.caps.render_device, self.default_feedback.clone())
        else {
            return;
        };
        let Some(surface) = self.layer_surface(view_id, hint.layer_id) else {
            tracing::debug!(
                view_id,
                layer_id = hint.layer_id,
                "scanout hint for no surface"
            );
            return;
        };
        let feedback = self.scanout_feedback(main, hint.dev, &hint.formats);
        tracing::debug!(
            view_id,
            layer_id = hint.layer_id,
            dev = hint.dev,
            formats = hint.formats.len(),
            tranche = feedback.is_some(),
            "scanout hint"
        );
        let hinted = &mut self.scanout.hinted;
        hinted.retain(|(s, _, _)| s.upgrade().is_ok_and(|s| s != surface));
        if let Some(feedback) = &feedback {
            hinted.push((surface.downgrade(), view_id, feedback.clone()));
        }
        set_feedback(&surface, feedback.as_ref().unwrap_or(&default));
    }

    /// View @p view_id no longer shows its surfaces: they go back to the
    /// default feedback.
    pub fn forget_scanout(&mut self, view_id: i32) {
        let Some(default) = self.default_feedback.clone() else {
            return;
        };
        self.scanout.hinted.retain(|(s, view, _)| {
            if *view != view_id {
                return true;
            }
            if let Ok(surface) = s.upgrade() {
                set_feedback(&surface, &default);
            }
            false
        });
    }

    /// Default feedback with a scanout tranche for @p dev ahead of it: the
    /// hinted formats the shell also imports. None when there are none.
    fn scanout_feedback(
        &mut self,
        main: u64,
        dev: u64,
        hinted: &[FormatModifier],
    ) -> Option<DmabufFeedback> {
        if let Some(built) = &self.scanout.last {
            if built.dev == dev && built.hinted == hinted {
                return built.feedback.clone();
            }
        }
        let tranche: Vec<FormatModifier> = hinted
            .iter()
            .filter(|f| self.caps.formats.contains(f))
            .copied()
            .collect();
        let feedback = if tranche.is_empty() {
            tracing::debug!(dev, "scanout hint: no format the shell also imports");
            None
        } else {
            DmabufFeedbackBuilder::new(main as libc::dev_t, formats(&self.caps.formats))
                .add_preference_tranche(
                    dev as libc::dev_t,
                    Some(TrancheFlags::Scanout),
                    formats(&tranche),
                )
                .build()
                .inspect_err(|e| tracing::warn!("scanout feedback: {e}"))
                .ok()
        };
        self.scanout.last = Some(Built {
            dev,
            hinted: hinted.to_vec(),
            feedback: feedback.clone(),
        });
        feedback
    }

    /// The surface of view @p view_id's scene that is layer @p layer_id.
    fn layer_surface(&self, view_id: i32, layer_id: u32) -> Option<WlSurface> {
        let mut found = None;
        for (root, _) in self.view_trees(view_id) {
            compositor::with_surface_tree_downward(
                &root,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |surface, states, _| {
                    if found.is_none() && crate::tree::layer_id_of(states) == Some(layer_id) {
                        found = Some(surface.clone());
                    }
                },
                |_, _, _| true,
            );
            if found.is_some() {
                break;
            }
        }
        found
    }
}

/// Send @p surface's feedback objects @p feedback, if it differs.
fn set_feedback(surface: &WlSurface, feedback: &DmabufFeedback) {
    compositor::with_states(surface, |states| {
        if let Some(state) = SurfaceDmabufFeedbackState::from_states(states) {
            state.set_feedback(feedback);
        }
    });
}
