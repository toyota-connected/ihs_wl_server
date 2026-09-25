// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Handing a view's layers to the shell, and getting the buffers back.
//!
//! Each submit dups the dma-buf fds (the registry consumes them whatever the
//! call returns) and holds a clone of every buffer until the shell is done
//! with it. The last clone dropped is what sends the client its
//! `wl_buffer.release`, so a buffer shown by several layers or several
//! submits goes back only once all of them are finished.
//!
//! The shell says it is done in one of two ways. A layer handed back a
//! release fence is held until the fence signals. One handed back none is
//! held until the shell reports a later frame of the view on screen: by then
//! it has stopped reading the earlier one.

use std::collections::HashMap;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};

use smithay::backend::allocator::Buffer as _;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::{self, SurfaceAttributes, TraversalAction};

use crate::buffers::BufferKey;
use crate::ffi::ihs::sys;
use crate::observe::{self, Observed};
use crate::state::State;
use crate::timing::Taken;
use crate::tree::{self, Held, LayerSpec};

/// Buffers held without a fence, per view, before the oldest goes back.
const MAX_UNFENCED: usize = 64;

/// Buffers the shell may still be reading, for one view.
#[derive(Default)]
pub struct Holds {
    next_key: u64,
    /// Held until their release fence signals.
    fenced: HashMap<u64, (Held, RegistrationToken)>,
    /// Held until a frame after the one that carried them is shown: (seq of
    /// that frame, buffer).
    unfenced: Vec<(u64, Held)>,
}

impl Holds {
    /// Hold @p buffer until @p fence signals; the fence is closed with it.
    fn hold_fenced(
        &mut self,
        handle: &LoopHandle<'static, State>,
        view_id: i32,
        buffer: Held,
        fence: OwnedFd,
    ) {
        self.next_key += 1;
        let key = self.next_key;
        let source = Generic::new(fence, Interest::READ, Mode::OneShot);
        match handle.insert_source(source, move |_, _, state| {
            if let Some(entry) = state.views.get_mut(&view_id) {
                entry.holds.fenced.remove(&key);
            }
            Ok(PostAction::Remove)
        }) {
            Ok(token) => {
                self.fenced.insert(key, (buffer, token));
            }
            Err(e) => {
                // Without a source nothing would ever release it: give it
                // back now rather than starve the client.
                tracing::warn!(view_id, "release fence source: {}", e.error);
            }
        }
    }

    fn hold_unfenced(&mut self, seq: u64, buffer: Held) {
        // A buffer submitted again is held for the later frame only. Without
        // this, a client that commits without waiting for frame callbacks
        // while its view is hidden -- so nothing is ever presented -- would
        // grow the list without bound.
        self.unfenced.retain(|(_, held)| *held != buffer);
        self.unfenced.push((seq, buffer));
        // And a hard bound, for one that cycles through fresh buffers: give
        // the oldest back. A shell that has not presented anything in this
        // many frames is not reading them.
        if self.unfenced.len() > MAX_UNFENCED {
            self.unfenced.remove(0);
        }
    }

    /// The view's frame @p seq is on screen: what earlier frames held
    /// without a fence is free.
    pub fn presented(&mut self, seq: u64) {
        self.unfenced.retain(|(held_seq, _)| *held_seq >= seq);
    }

    /// Give everything back (the view is gone or unbound).
    pub fn clear(&mut self, handle: &LoopHandle<'static, State>) {
        for (_, (_, token)) in self.fenced.drain() {
            handle.remove(token);
        }
        self.unfenced.clear();
    }

    #[cfg(test)]
    fn held(&self) -> usize {
        self.fenced.len() + self.unfenced.len()
    }
}

/// 16.16 fixed point, clamped to the field's range.
fn fixed(v: f64) -> i64 {
    (v * 65536.0).round() as i64
}

/// The frame description for @p spec, with its planes' fds dup'd. None when
/// a dup fails, with nothing left open.
fn frame_for(spec: &LayerSpec, buffer_id: u32) -> Option<sys::IhsFrame> {
    let format = spec.dmabuf.format();
    let (width, height) = tree::dmabuf_size(&spec.dmabuf);
    let mut frame = sys::IhsFrame {
        struct_size: std::mem::size_of::<sys::IhsFrame>(),
        format: sys::IhsFormatModifier {
            fourcc: format.code as u32,
            reserved: 0,
            modifier: u64::from(format.modifier),
        },
        width,
        height,
        plane_fd: [-1; 4],
        buffer_id,
        ..Default::default()
    };
    let planes = spec
        .dmabuf
        .handles()
        .zip(spec.dmabuf.offsets())
        .zip(spec.dmabuf.strides())
        .take(4);
    for (i, ((fd, offset), stride)) in planes.enumerate() {
        match fd.try_clone_to_owned() {
            Ok(owned) => frame.plane_fd[i] = owned.into_raw_fd(),
            Err(e) => {
                tracing::warn!("dup dma-buf plane: {e}");
                close_frame(&frame);
                return None;
            }
        }
        frame.plane_offset[i] = offset;
        frame.plane_stride[i] = stride;
        frame.plane_count = i as u32 + 1;
    }
    Some(frame)
}

fn close_frame(frame: &sys::IhsFrame) {
    for &fd in &frame.plane_fd {
        if fd >= 0 {
            // SAFETY: a dup this module made and still owns.
            drop(unsafe { OwnedFd::from_raw_fd(fd) });
        }
    }
}

/// @p v, a logical coordinate, in physical pixels.
fn physical(v: i32, dpr: f64) -> i64 {
    (v as f64 * dpr).round() as i64
}

fn layer_for(spec: &LayerSpec, frame: &sys::IhsFrame, dpr: f64) -> sys::IhsLayer {
    let clamp_i = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    let clamp_u = |v: i64| v.clamp(0, u32::MAX as i64) as u32;
    sys::IhsLayer {
        struct_size: std::mem::size_of::<sys::IhsLayer>(),
        frame,
        // Only ready buffers reach the shell; there is nothing to wait on.
        acquire_fence_fd: -1,
        layer_id: spec.layer_id,
        src_x: clamp_i(fixed(spec.src.loc.x)),
        src_y: clamp_i(fixed(spec.src.loc.y)),
        src_w: clamp_u(fixed(spec.src.size.w)),
        src_h: clamp_u(fixed(spec.src.size.h)),
        // Edges round, not sizes, so neighbors stay flush.
        dst_x: clamp_i(physical(spec.dst.loc.x, dpr)),
        dst_y: clamp_i(physical(spec.dst.loc.y, dpr)),
        dst_w: clamp_u(
            physical(spec.dst.loc.x + spec.dst.size.w, dpr) - physical(spec.dst.loc.x, dpr),
        ),
        dst_h: clamp_u(
            physical(spec.dst.loc.y + spec.dst.size.h, dpr) - physical(spec.dst.loc.y, dpr),
        ),
        transform: spec.transform,
        opaque: spec.opaque as u8,
        content_type: 0,
        reserved: [0; 2],
    }
}

impl State {
    /// Hand the view's toplevel tree, as it now stands, to the shell, with
    /// what its updates wait on the screen for.
    pub fn submit_view(&mut self, view_id: i32) {
        let Some(entry) = self.views.get(&view_id) else {
            return;
        };
        let Some(toplevel) = entry.toplevel.and_then(|id| self.toplevels.by_id.get(&id)) else {
            return;
        };
        let root = toplevel.surface.wl_surface().clone();
        // Surfaces new to the scene learn its scale.
        self.scale_tree(view_id);
        let trees = self.view_trees(view_id);
        let taken = Taken::from_trees(trees.iter().map(|(s, _)| s));
        let Some((seq, shown)) = self.submit_tree(view_id, &trees) else {
            self.hold_loose(root.client(), taken.not_shown());
            return;
        };
        let has_barriers = taken.has_barriers();
        let entry = self.views.get_mut(&view_id).unwrap();
        if entry.frames.push(seq, shown, taken) {
            if let Some(client) = root.client() {
                self.unblock(&client);
            }
        }
        if has_barriers {
            self.watch_frames(view_id);
        }
    }

    /// Submit the layers of the scene @p trees; the seq it went as, and the
    /// (layer id, content generation) of each layer.
    fn submit_tree(
        &mut self,
        view_id: i32,
        trees: &[crate::popups::Tree],
    ) -> Option<(u64, Vec<(u32, u64)>)> {
        let built = tree::build(trees, &mut self.stager, &mut self.egl);
        for id in self.egl.reap() {
            self.retire_key(&BufferKey::Client(id));
        }
        for uid in &built.retired {
            self.retire_key(&BufferKey::Staged(*uid));
        }
        if built.skipped > 0 {
            tracing::debug!(view_id, n = built.skipped, "surfaces with nothing to show");
        }
        if built.layers.is_empty() {
            // Nothing the shell can show (only shared-memory surfaces, say):
            // show nothing rather than the frame before.
            self.clear_view(view_id);
            return None;
        }
        if built.layers.len() > sys::IHS_PV_MAX_LAYERS as usize {
            tracing::warn!(
                view_id,
                n = built.layers.len(),
                max = sys::IHS_PV_MAX_LAYERS,
                "more surfaces than the shell takes; showing the bottom ones"
            );
        }
        let specs: Vec<LayerSpec> = built
            .layers
            .into_iter()
            .take(sys::IHS_PV_MAX_LAYERS as usize)
            .collect();

        let mut frames: Vec<sys::IhsFrame> = Vec::with_capacity(specs.len());
        for spec in &specs {
            let id = self.buffers.id_for(&spec.key, view_id);
            match frame_for(spec, id) {
                Some(frame) => frames.push(frame),
                None => {
                    frames.iter().for_each(close_frame);
                    return None;
                }
            }
        }
        // Built after every frame is in place: the layers point into
        // `frames`, which must not move again.
        let dpr = self.views.get(&view_id).map_or(1.0, |v| v.dpr);
        let layers: Vec<sys::IhsLayer> = specs
            .iter()
            .zip(frames.iter())
            .map(|(spec, frame)| layer_for(spec, frame, dpr))
            .collect();
        let mut release = vec![-1; layers.len()];

        let entry = self.views.get_mut(&view_id).unwrap();
        let seq = entry.seq + 1;
        let rc = {
            let link = entry.handle.link.lock().unwrap_or_else(|e| e.into_inner());
            match link.as_ref() {
                // SAFETY: the lock keeps dispose from completing meanwhile.
                Some(link) => unsafe {
                    sys::ihs_pv_submit_layers(
                        link.view,
                        layers.as_ptr(),
                        layers.len(),
                        seq,
                        release.as_mut_ptr(),
                    )
                },
                None => {
                    // Disposed since the command queue last looked.
                    frames.iter().for_each(close_frame);
                    return None;
                }
            }
        };
        // The registry owns every plane fd from here, whatever it returned.
        if rc != sys::IHS_PV_OK {
            tracing::warn!(view_id, rc, "ihs_pv_submit_layers failed");
            for fd in release.into_iter().filter(|&fd| fd >= 0) {
                drop(unsafe { OwnedFd::from_raw_fd(fd) });
            }
            return None;
        }
        entry.seq = seq;
        entry.cleared = false;
        let n = specs.len();
        let shown: Vec<(u32, u64)> = specs.iter().map(|s| (s.layer_id, s.generation)).collect();
        for (spec, fd) in specs.into_iter().zip(release) {
            if fd >= 0 {
                // SAFETY: the registry hands ownership of the fence over.
                let fence = unsafe { OwnedFd::from_raw_fd(fd) };
                entry
                    .holds
                    .hold_fenced(&self.loop_handle, view_id, spec.held, fence);
            } else {
                entry.holds.hold_unfenced(seq, spec.held);
            }
        }
        observe::emit(Observed::Submitted {
            view_id,
            seq,
            layers: n,
        });
        Some((seq, shown))
    }

    /// Show nothing in @p view_id: its toplevel is gone, or has nothing the
    /// shell can show. Once; a view already showing nothing is left alone.
    pub fn clear_view(&mut self, view_id: i32) {
        let Some(entry) = self.views.get_mut(&view_id) else {
            return;
        };
        if entry.cleared || entry.seq == 0 {
            // Nothing was ever shown, or nothing is now.
            return;
        }
        let seq = entry.seq + 1;
        let rc = {
            let link = entry.handle.link.lock().unwrap_or_else(|e| e.into_inner());
            let Some(link) = link.as_ref() else {
                return;
            };
            // SAFETY: as for submit, under the view's lock. No layers: NULL.
            unsafe {
                sys::ihs_pv_submit_layers(link.view, std::ptr::null(), 0, seq, std::ptr::null_mut())
            }
        };
        if rc != sys::IHS_PV_OK {
            tracing::warn!(view_id, rc, "clearing the view failed");
            return;
        }
        entry.seq = seq;
        entry.cleared = true;
        observe::emit(Observed::Submitted {
            view_id,
            seq,
            layers: 0,
        });
    }

    /// The client destroyed a buffer: every view it was shown in drops its
    /// import of it.
    pub fn retire_buffer(
        &mut self,
        buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    ) {
        self.retire_key(&BufferKey::client(buffer));
    }

    /// The buffer @p key names is gone: every view it was shown in drops its
    /// import of it.
    pub fn retire_key(&mut self, key: &BufferKey) {
        let Some((buffer_id, views)) = self.buffers.destroyed(key) else {
            return;
        };
        for view_id in views {
            let Some(entry) = self.views.get(&view_id) else {
                continue;
            };
            let link = entry.handle.link.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(link) = link.as_ref() {
                // SAFETY: as for submit, under the view's lock.
                let rc = unsafe { sys::ihs_pv_retire_buffer(link.view, buffer_id) };
                if rc != sys::IHS_PV_OK {
                    tracing::debug!(view_id, buffer_id, rc, "ihs_pv_retire_buffer");
                }
            }
            observe::emit(Observed::Retired { view_id, buffer_id });
        }
    }
}

/// Tell every surface of the tree its frame was shown: the clients may draw
/// the next one. @p ust_ns is CLOCK_MONOTONIC; the callback's time is
/// milliseconds on any base.
pub fn send_frame_callbacks(root: &WlSurface, ust_ns: u64) {
    let time_ms = (ust_ns / 1_000_000) as u32;
    compositor::with_surface_tree_downward(
        root,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |_, states, _| {
            let callbacks = std::mem::take(
                &mut states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .frame_callbacks,
            );
            for callback in callbacks {
                callback.done(time_ms);
            }
        },
        |_, _, _| true,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point() {
        assert_eq!(fixed(1.0), 65536);
        assert_eq!(fixed(0.5), 32768);
        assert_eq!(fixed(1.0 / 256.0), 256);
    }

    #[test]
    fn empty_holds() {
        let holds = Holds::default();
        assert_eq!(holds.held(), 0);
        assert!(MAX_UNFENCED >= 2 * sys::IHS_PV_MAX_LAYERS as usize);
    }
}
