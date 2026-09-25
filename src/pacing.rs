// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Releasing the content updates that wait on the screen: fifo barriers and
//! commit timers (see timing.rs), from the shell's reports and, where there
//! are none, from a clock ticking at the display's refresh.

use std::time::Duration;

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::utils::{Monotonic, Time};
use smithay::wayland::commit_timing::CommitTimerBarrierStateUserData;
use smithay::wayland::compositor::{self, Barrier, CompositorHandler};

use crate::state::State;
use crate::timing::{self, now_ns, Report, Taken};

/// Fifo barriers of updates no view shows held for the clock, at most. A
/// client committing faster than that between two ticks gets the oldest
/// cleared early.
const MAX_LOOSE: usize = 256;

/// A fifo barrier of an update no view shows, cleared on the clock so its
/// client keeps a steady pace.
pub struct Loose {
    barrier: Barrier,
    client: Client,
    due_ns: u64,
}

/// Unblock each client once.
fn unblock_all(state: &mut State, clients: Vec<Client>) {
    let mut seen = Vec::with_capacity(clients.len());
    for client in clients {
        if !seen.contains(&client.id()) {
            seen.push(client.id());
            state.unblock(&client);
        }
    }
}

impl State {
    /// Barriers of @p client's were cleared: apply what waited on them.
    pub fn unblock(&mut self, client: &Client) {
        let dh = self.dh.clone();
        self.client_compositor_state(client)
            .blocker_cleared(self, &dh);
    }

    /// The client of the toplevel @p view_id shows.
    fn view_client(&self, view_id: i32) -> Option<Client> {
        let id = self.views.get(&view_id)?.toplevel?;
        self.toplevels.by_id.get(&id)?.surface.wl_surface().client()
    }

    /// The tree rooted at @p root committed, but no view shows it.
    pub fn not_shown(&mut self, root: &WlSurface) {
        let barriers = Taken::from_trees([root]).not_shown();
        self.hold_loose(root.client(), barriers);
    }

    /// Clear @p barriers at the next refresh.
    pub fn hold_loose(&mut self, client: Option<Client>, barriers: Vec<Barrier>) {
        if barriers.is_empty() {
            return;
        }
        let Some(client) = client else {
            barriers.iter().for_each(Barrier::signal);
            return;
        };
        let due_ns = self.clock.next_vblank(now_ns());
        self.loose.extend(barriers.into_iter().map(|barrier| Loose {
            barrier,
            client: client.clone(),
            due_ns,
        }));
        if self.loose.len() > MAX_LOOSE {
            let excess = self.loose.len() - MAX_LOOSE;
            for l in self.loose.drain(..excess) {
                l.barrier.signal();
            }
        }
        self.schedule_tick(due_ns);
    }

    /// Called before @p surface's commit is applied: when it carries a
    /// target time, watch it until it is due.
    pub fn watch_target(&mut self, surface: &WlSurface) {
        if !timing::has_target(surface) {
            return;
        }
        if !self.timed.iter().any(|w| w == surface) {
            self.timed.push(surface.downgrade());
        }
        // The barrier is registered right after this hook; work out when it
        // is due on the next pass.
        self.schedule_tick(now_ns());
    }

    /// Frame @p report.seq of @p view_id is on screen.
    pub fn frame_presented(&mut self, view_id: i32, report: Report) {
        self.clock.update(report.ust_ns, report.refresh_ns);
        let output = &self.output;
        let Some(entry) = self.views.get_mut(&view_id) else {
            return;
        };
        entry.holds.presented(report.seq);
        let released = entry.frames.presented(&report, output);
        let trees = self.view_trees(view_id);
        for (root, _) in &trees {
            crate::submit::send_frame_callbacks(root, report.ust_ns);
        }
        if released {
            if let Some(client) = trees.first().and_then(|(root, _)| root.client()) {
                self.unblock(&client);
            }
        }
    }

    /// The view left (@p suspended) or re-entered the scene. Off screen,
    /// nothing will be reported: its barriers clear on the clock.
    pub fn view_suspended(&mut self, view_id: i32, suspended: bool) {
        let Some(entry) = self.views.get_mut(&view_id) else {
            return;
        };
        entry.suspended = suspended;
        if suspended {
            self.dismiss_popups(view_id);
        }
        let Some(entry) = self.views.get_mut(&view_id) else {
            return;
        };
        if suspended && entry.frames.next_stale().is_some() {
            self.schedule_tick(self.clock.next_vblank(now_ns()));
        }
    }

    /// A frame of @p view_id holding barriers was submitted: make sure they
    /// clear even if it is never reported.
    pub fn watch_frames(&mut self, view_id: i32) {
        let Some(entry) = self.views.get(&view_id) else {
            return;
        };
        let at = if entry.suspended {
            Some(self.clock.next_vblank(now_ns()))
        } else {
            entry.frames.next_stale()
        };
        if let Some(at) = at {
            self.schedule_tick(at);
        }
    }

    /// The view shows nothing more: clear what its frames held.
    pub fn drop_frames(&mut self, view_id: i32) {
        let client = self.view_client(view_id);
        let Some(entry) = self.views.get_mut(&view_id) else {
            return;
        };
        if entry.frames.clear() {
            if let Some(client) = client {
                self.unblock(&client);
            }
        }
    }

    /// Run the clock at @p at_ns, unless it already runs sooner.
    fn schedule_tick(&mut self, at_ns: u64) {
        if let Some((token, when)) = self.tick {
            if when <= at_ns {
                return;
            }
            self.loop_handle.remove(token);
            self.tick = None;
        }
        let delay = Duration::from_nanos(at_ns.saturating_sub(now_ns()));
        match self
            .loop_handle
            .insert_source(Timer::from_duration(delay), |_, _, state| {
                state.tick = None;
                state.on_tick();
                TimeoutAction::Drop
            }) {
            Ok(token) => self.tick = Some((token, at_ns)),
            // Nothing would release what waits; better early than never.
            Err(e) => {
                tracing::warn!("frame clock timer: {e}");
                self.release_everything();
            }
        }
    }

    fn on_tick(&mut self) {
        let now = now_ns();
        let vblank = self.clock.next_vblank(now);
        let mut clients: Vec<Client> = Vec::new();
        let mut next: Option<u64> = None;
        let mut wake = |at: u64| next = Some(next.map_or(at, |n: u64| n.min(at)));

        // Commit timers: apply what the next vblank can show.
        let due = timing::timestamp(self.clock.due(now));
        let clock = self.clock;
        self.timed.retain(|weak| {
            let Ok(surface) = weak.upgrade() else {
                return false;
            };
            let (released, pending) = compositor::with_states(&surface, |states| {
                let Some(timers) = states.data_map.get::<CommitTimerBarrierStateUserData>() else {
                    return (false, None);
                };
                let mut timers = timers.lock().unwrap();
                (timers.signal_until(due), timers.next_deadline())
            });
            if released {
                clients.extend(surface.client());
            }
            match pending {
                Some(target) => {
                    let target: Duration = Time::<Monotonic>::from(target).into();
                    wake(clock.release_at(target.as_nanos() as u64));
                    true
                }
                None => false,
            }
        });

        // Barriers of updates nobody shows.
        self.loose.retain(|l| {
            if l.due_ns <= now {
                l.barrier.signal();
                clients.push(l.client.clone());
                false
            } else {
                wake(l.due_ns);
                true
            }
        });

        // Frames that went unreported.
        let stale_before = now.saturating_sub(timing::STALE_NS);
        let mut released_views = Vec::new();
        for (&view_id, entry) in self.views.iter_mut() {
            let released = if entry.suspended {
                entry.frames.release_all()
            } else {
                entry.frames.release_older(stale_before)
            };
            if released {
                released_views.push(view_id);
            }
            if let Some(at) = entry.frames.next_stale() {
                wake(if entry.suspended { vblank } else { at });
            }
        }
        clients.extend(
            released_views
                .into_iter()
                .filter_map(|id| self.view_client(id)),
        );

        unblock_all(self, clients);
        if let Some(at) = next {
            // Never a busy loop, whatever rounding says.
            self.schedule_tick(at.max(now + 100_000));
        }
    }

    /// The clock could not run: release everything now.
    fn release_everything(&mut self) {
        let mut clients: Vec<Client> = Vec::new();
        for l in self.loose.drain(..) {
            l.barrier.signal();
            clients.push(l.client);
        }
        for weak in self.timed.drain(..) {
            if let Ok(surface) = weak.upgrade() {
                compositor::with_states(&surface, |states| {
                    if let Some(timers) = states.data_map.get::<CommitTimerBarrierStateUserData>() {
                        timers
                            .lock()
                            .unwrap()
                            .signal_until(timing::timestamp(u64::MAX));
                    }
                });
                clients.extend(surface.client());
            }
        }
        let views: Vec<i32> = self.views.keys().copied().collect();
        for view_id in views {
            if self.views.get_mut(&view_id).unwrap().frames.release_all() {
                clients.extend(self.view_client(view_id));
            }
        }
        unblock_all(self, clients);
    }
}
