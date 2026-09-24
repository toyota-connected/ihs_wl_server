// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! When the clients' content updates reach the screen, and when the next may
//! be applied.
//!
//! The shell reports each frame of a view it shows (`presented`), once, on
//! the refresh that first shows it. That one report drives three protocols:
//!
//! - wp_presentation: each surface of the frame hears when its content was
//!   shown -- or that it never was, when a later update replaced it first.
//! - wp_fifo_v1: a barrier set by a content update clears once that update
//!   has been shown.
//! - wp_commit_timing_v1: an update targeted at a time is applied a refresh
//!   ahead of the first vblank at or after it.
//!
//! A view that is not on screen gets no reports, so a clock phase-locked to
//! the last one stands in: it ticks at the display's refresh while there is
//! anything to release, clearing the fifo barriers of updates nobody shows
//! and applying timed updates when they are due.

use std::cell::Cell;
use std::collections::VecDeque;
use std::time::Duration;

use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Clock, Monotonic, Time};
use smithay::wayland::commit_timing::{CommitTimerStateUserData, Timestamp};
use smithay::wayland::compositor::{self, Barrier, TraversalAction};
use smithay::wayland::fifo::FifoBarrierCachedState;
use smithay::wayland::presentation::{
    PresentationFeedbackCachedState, PresentationFeedbackCallback, Refresh,
};

/// Assumed until the shell reports the display's own.
const DEFAULT_REFRESH_NS: u64 = 16_666_667;

/// How early a vblank may fall and still count as "at" a target time:
/// clock rounding between the client's arithmetic and ours.
const TARGET_SLACK_NS: u64 = 500_000;

/// Frames of a view held for their report, before the oldest is taken as
/// never shown. Far more than the shell keeps in flight.
const MAX_IN_FLIGHT: usize = 16;

/// How long a frame of a view on screen may go unreported before its fifo
/// barriers are cleared anyway, so a client can always make progress.
pub const STALE_NS: u64 = 250_000_000;

/// CLOCK_MONOTONIC, which the shell's reports and wp_presentation use.
pub fn now_ns() -> u64 {
    let now: Duration = Clock::<Monotonic>::new().now().into();
    now.as_nanos() as u64
}

/// The display's refresh cycle, as last reported.
#[derive(Clone, Copy, Debug)]
pub struct FrameClock {
    ust_ns: u64,
    refresh_ns: u64,
}

impl Default for FrameClock {
    fn default() -> Self {
        FrameClock {
            ust_ns: 0,
            refresh_ns: DEFAULT_REFRESH_NS,
        }
    }
}

impl FrameClock {
    /// A frame started to show at @p ust_ns; @p refresh_ns is 0 when unknown.
    pub fn update(&mut self, ust_ns: u64, refresh_ns: u32) {
        if ust_ns > self.ust_ns {
            self.ust_ns = ust_ns;
        }
        if refresh_ns > 0 {
            self.refresh_ns = refresh_ns as u64;
        }
    }

    /// The first predicted vblank after @p t.
    pub fn next_vblank(&self, t: u64) -> u64 {
        if t < self.ust_ns {
            return self.ust_ns;
        }
        let periods = (t - self.ust_ns) / self.refresh_ns + 1;
        self.ust_ns
            .saturating_add(periods.saturating_mul(self.refresh_ns))
    }

    /// The latest target an update applied at @p now_ns can meet: that of
    /// the next vblank, which is when it will be shown.
    pub fn due(&self, now_ns: u64) -> u64 {
        self.next_vblank(now_ns).saturating_add(TARGET_SLACK_NS)
    }

    /// The latest time an update may be applied to be shown at the first
    /// vblank at or after @p target_ns: one refresh before that vblank.
    pub fn release_at(&self, target_ns: u64) -> u64 {
        let target = target_ns.saturating_sub(TARGET_SLACK_NS);
        let vblank = self.next_vblank(target.saturating_sub(1));
        vblank.saturating_sub(self.refresh_ns)
    }
}

/// What the shell reported for a shown frame.
#[derive(Clone, Copy, Debug)]
pub struct Report {
    pub seq: u64,
    pub ust_ns: u64,
    pub refresh_ns: u32,
    pub msc: u64,
    pub flags: u32,
}

/// A surface's content generation: bumped by each of its content updates,
/// so a frame can tell whether the content it shows is still the one a
/// feedback was asked for.
struct Generation(Cell<u64>);

/// The content generation of @p states' surface.
pub fn generation(states: &compositor::SurfaceData) -> u64 {
    states
        .data_map
        .get::<Generation>()
        .map(|g| g.0.get())
        .unwrap_or(0)
}

/// @p surface's content update was applied.
pub fn bump_generation(surface: &WlSurface) {
    compositor::with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing(|| Generation(Cell::new(0)));
        let g = &states.data_map.get::<Generation>().unwrap().0;
        g.set(g.get() + 1);
    });
}

/// Whether @p surface's pending update carries a commit-timing target.
pub fn has_target(surface: &WlSurface) -> bool {
    compositor::with_states(surface, |states| {
        states
            .data_map
            .get::<CommitTimerStateUserData>()
            .is_some_and(|s| s.borrow().timestamp.is_some())
    })
}

/// A target time as commit timing compares them.
pub fn timestamp(ns: u64) -> Timestamp {
    Time::<Monotonic>::from(Duration::from_nanos(ns)).into()
}

/// What a tree's current content updates wait on the screen for: their
/// presentation feedback and fifo barriers, taken as the tree is submitted.
#[derive(Default)]
pub struct Taken {
    /// (layer id, content generation, callback).
    feedback: Vec<(u32, u64, PresentationFeedbackCallback)>,
    barriers: Vec<Barrier>,
}

impl Taken {
    /// Take them from every surface of the tree rooted at @p root.
    pub fn from_tree(root: &WlSurface) -> Self {
        let mut taken = Taken::default();
        compositor::with_surface_tree_downward(
            root,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |_, states, _| {
                let callbacks = std::mem::take(
                    &mut states
                        .cached_state
                        .get::<PresentationFeedbackCachedState>()
                        .current()
                        .callbacks,
                );
                if !callbacks.is_empty() {
                    let layer_id = crate::tree::layer_id(states);
                    let generation = generation(states);
                    taken
                        .feedback
                        .extend(callbacks.into_iter().map(|cb| (layer_id, generation, cb)));
                }
                let barrier = states
                    .cached_state
                    .get::<FifoBarrierCachedState>()
                    .current()
                    .barrier
                    .take();
                taken.barriers.extend(barrier);
            },
            |_, _, _| true,
        );
        taken
    }

    pub fn has_barriers(&self) -> bool {
        !self.barriers.is_empty()
    }

    /// Nothing of it will be shown: the feedback is discarded, and the
    /// barriers are handed back to be cleared on the clock.
    pub fn not_shown(self) -> Vec<Barrier> {
        for (_, _, cb) in self.feedback {
            cb.discarded();
        }
        self.barriers
    }
}

/// A submitted frame, until the shell reports it or it is taken as never
/// shown.
struct Frame {
    seq: u64,
    submitted_ns: u64,
    /// The content generation of each layer it shows, by layer id.
    shown: Vec<(u32, u64)>,
    feedback: Vec<(u32, u64, PresentationFeedbackCallback)>,
    barriers: Vec<Barrier>,
}

impl Frame {
    fn shows(&self, layer_id: u32, generation: u64) -> bool {
        self.shown.contains(&(layer_id, generation))
    }

    /// Clear its barriers; whether it had any.
    fn release(&mut self) -> bool {
        let any = !self.barriers.is_empty();
        for barrier in self.barriers.drain(..) {
            barrier.signal();
        }
        any
    }

    /// Never shown: discard its feedback and clear its barriers.
    fn drop_unshown(mut self) -> bool {
        for (_, _, cb) in self.feedback.drain(..) {
            cb.discarded();
        }
        self.release()
    }
}

/// A view's frames awaiting the shell's report, oldest first.
#[derive(Default)]
pub struct Frames {
    frames: VecDeque<Frame>,
}

impl Frames {
    /// Frame @p seq was submitted, showing @p shown ((layer id, content
    /// generation) of each layer). Feedback for surfaces it does not show is
    /// discarded now. Returns whether barriers were cleared to make room.
    pub fn push(&mut self, seq: u64, shown: Vec<(u32, u64)>, taken: Taken) -> bool {
        let mut frame = Frame {
            seq,
            submitted_ns: now_ns(),
            shown,
            feedback: Vec::with_capacity(taken.feedback.len()),
            barriers: taken.barriers,
        };
        for (layer_id, generation, cb) in taken.feedback {
            if frame.shows(layer_id, generation) {
                frame.feedback.push((layer_id, generation, cb));
            } else {
                cb.discarded();
            }
        }
        self.frames.push_back(frame);
        let mut released = false;
        while self.frames.len() > MAX_IN_FLIGHT {
            released |= self.frames.pop_front().unwrap().drop_unshown();
        }
        released
    }

    /// The shell showed frame @p report.seq. Frames before it were replaced
    /// before they were shown, but content of theirs still in it was shown
    /// with it. Returns whether any barrier was cleared.
    pub fn presented(&mut self, report: &Report, output: &Output) -> bool {
        // Not found when it was already taken as never shown: then nothing
        // of the frames before it counts as shown either.
        let shown = self
            .frames
            .iter()
            .find(|f| f.seq == report.seq)
            .map(|f| f.shown.clone())
            .unwrap_or_default();
        let time = Duration::from_nanos(report.ust_ns);
        let refresh = match report.refresh_ns {
            0 => Refresh::Unknown,
            ns => Refresh::fixed(Duration::from_nanos(ns as u64)),
        };
        let kind = wp_presentation_feedback::Kind::from_bits_truncate(report.flags);
        let mut released = false;
        while self.frames.front().is_some_and(|f| f.seq <= report.seq) {
            let mut frame = self.frames.pop_front().unwrap();
            for (layer_id, generation, cb) in frame.feedback.drain(..) {
                if shown.contains(&(layer_id, generation)) {
                    cb.presented(output, time, refresh, report.msc, kind);
                } else {
                    cb.discarded();
                }
            }
            released |= frame.release();
        }
        released
    }

    /// Clear the barriers of frames submitted before @p before_ns, keeping
    /// them for their report. Returns whether any was cleared.
    pub fn release_older(&mut self, before_ns: u64) -> bool {
        let mut released = false;
        for frame in self.frames.iter_mut() {
            if frame.submitted_ns < before_ns {
                released |= frame.release();
            }
        }
        released
    }

    /// Clear every barrier, keeping the frames for their report.
    pub fn release_all(&mut self) -> bool {
        self.release_older(u64::MAX)
    }

    /// When the oldest frame still holding a barrier goes stale.
    pub fn next_stale(&self) -> Option<u64> {
        self.frames
            .iter()
            .find(|f| !f.barriers.is_empty())
            .map(|f| f.submitted_ns.saturating_add(STALE_NS))
    }

    /// The view is gone or unbound: nothing more will be shown.
    pub fn clear(&mut self) -> bool {
        let mut released = false;
        for frame in self.frames.drain(..) {
            released |= frame.drop_unshown();
        }
        released
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vblank_prediction() {
        let mut c = FrameClock::default();
        c.update(1_000, 100);
        assert_eq!(c.next_vblank(1_000), 1_100);
        assert_eq!(c.next_vblank(1_050), 1_100);
        assert_eq!(c.next_vblank(1_099), 1_100);
        assert_eq!(c.next_vblank(500), 1_000);
        // A report of an older frame never moves the phase back.
        c.update(900, 0);
        assert_eq!(c.next_vblank(1_000), 1_100);
        assert_eq!(c.refresh_ns, 100);
    }

    #[test]
    fn release_time() {
        let mut c = FrameClock::default();
        c.update(10_000_000, 1_000_000);
        // A target on a vblank: released a refresh before it.
        assert_eq!(c.release_at(15_000_000), 14_000_000);
        // Just after one: the next vblank is the first at or after it.
        assert_eq!(c.release_at(15_600_000), 15_000_000);
        // Within the slack of one counts as on it.
        assert_eq!(c.release_at(15_400_000), 14_000_000);
    }
}
