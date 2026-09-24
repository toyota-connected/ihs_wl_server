// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! A toplevel's committed surface tree, as the layer list the shell draws.
//!
//! Every surface with a dma-buf attached becomes one layer, bottom to top in
//! the stacking order the client asked for: the buffer, the part of it shown
//! (the viewport source, in the buffer's own pixels), where it lands in the
//! view, and its orientation. The shell treats each exactly like a Flutter
//! layer, compositing it or putting it on a plane of its own.

use std::sync::atomic::{AtomicU32, Ordering};

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::Buffer as _;
use smithay::backend::renderer::utils::{Buffer, RendererSurfaceStateUserData};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Transform};
use smithay::wayland::compositor::{self, TraversalAction};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::shell::xdg::SurfaceCachedState;

/// One layer of a view, before the buffer ids and fds are filled in.
pub struct LayerSpec {
    /// Stable for the surface's life, so a layer keeps its plane while the
    /// tree around it changes.
    pub layer_id: u32,
    /// Held until the shell is done with it; the last clone dropped releases
    /// the buffer to the client.
    pub buffer: Buffer,
    pub dmabuf: Dmabuf,
    /// The part of the buffer shown, in its own pixels, before `transform`.
    pub src: Rectangle<f64, smithay::utils::Buffer>,
    /// Where it lands, in view-local pixels.
    pub dst: Rectangle<i32, Logical>,
    /// wl_output.transform values, which IhsTransform shares.
    pub transform: u32,
    pub opaque: bool,
}

/// A surface's layer id, kept in its data map.
struct LayerId(u32);

fn layer_id(states: &compositor::SurfaceData) -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    states
        .data_map
        .insert_if_missing_threadsafe(|| LayerId(NEXT.fetch_add(1, Ordering::Relaxed)));
    states.data_map.get::<LayerId>().unwrap().0
}

fn transform_value(t: Transform) -> u32 {
    match t {
        Transform::Normal => 0,
        Transform::_90 => 1,
        Transform::_180 => 2,
        Transform::_270 => 3,
        Transform::Flipped => 4,
        Transform::Flipped90 => 5,
        Transform::Flipped180 => 6,
        Transform::Flipped270 => 7,
    }
}

/// The toplevel's window geometry origin: where its content starts within
/// the surface, so client-side shadows and the like fall outside the view.
fn geometry_origin(root: &WlSurface) -> Point<i32, Logical> {
    compositor::with_states(root, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .geometry
            .map(|g| g.loc)
            .unwrap_or_default()
    })
}

/// How many surfaces of the tree have a buffer that is not a dma-buf. They
/// are left out of the layers (shared-memory buffers are not handled yet).
pub struct Built {
    pub layers: Vec<LayerSpec>,
    pub skipped_non_dmabuf: usize,
}

/// The layers of the tree rooted at @p root, bottom to top.
pub fn build(root: &WlSurface) -> Built {
    let origin = geometry_origin(root);
    let mut built = Built {
        layers: Vec::new(),
        skipped_non_dmabuf: 0,
    };
    let start: Point<i32, Logical> = (-origin.x, -origin.y).into();
    compositor::with_surface_tree_upward(
        root,
        start,
        |_, states, location| {
            let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() else {
                return TraversalAction::SkipChildren;
            };
            match data.lock().unwrap().view() {
                Some(view) => TraversalAction::DoChildren(*location + view.offset),
                None => TraversalAction::SkipChildren,
            }
        },
        |_, states, location| {
            let Some(data) = states.data_map.get::<RendererSurfaceStateUserData>() else {
                return;
            };
            let data = data.lock().unwrap();
            let (Some(view), Some(buffer), Some(buffer_size)) =
                (data.view(), data.buffer(), data.buffer_size())
            else {
                return;
            };
            let Ok(dmabuf) = get_dmabuf(buffer) else {
                built.skipped_non_dmabuf += 1;
                return;
            };
            let transform = data.buffer_transform();
            let src =
                view.src
                    .to_buffer(data.buffer_scale() as f64, transform, &buffer_size.to_f64());
            let dst = Rectangle::new(*location + view.offset, view.dst);
            // Opaque when the client says every pixel of the surface is: its
            // opaque region covers the whole of it.
            let whole = Rectangle::from_size(view.dst);
            let opaque = data
                .opaque_regions()
                .is_some_and(|r| r.iter().any(|o| o.contains_rect(whole)));
            built.layers.push(LayerSpec {
                layer_id: layer_id(states),
                buffer: buffer.clone(),
                dmabuf: dmabuf.clone(),
                src,
                dst,
                transform: transform_value(transform),
                opaque,
            });
        },
        |_, _, _| true,
    );
    built
}

/// The size of a dma-buf, for the frame description.
pub fn dmabuf_size(dmabuf: &Dmabuf) -> (u32, u32) {
    let size = dmabuf.size();
    (size.w.max(0) as u32, size.h.max(0) as u32)
}
