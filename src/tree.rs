// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! A toplevel's committed surface tree, as the layer list the shell draws.
//!
//! Every surface with a buffer attached becomes one layer, bottom to top in
//! the stacking order the client asked for: the buffer, the part of it shown
//! (the viewport source, in the buffer's own pixels), where it lands in the
//! view, and its orientation. The shell treats each exactly like a Flutter
//! layer, compositing it or putting it on a plane of its own. A shared-memory
//! buffer goes as the staging slot it is copied into (staging.rs).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::Buffer as _;
use smithay::backend::renderer::utils::{Buffer, RendererSurfaceStateUserData};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Transform};
use smithay::wayland::compositor::{self, TraversalAction};
use smithay::wayland::dmabuf::get_dmabuf;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use crate::buffers::BufferKey;
use crate::egl_display::Content;
use crate::staging::{self, Stager};

/// What keeps a layer's buffer from the client until the shell is done.
#[derive(Clone)]
pub enum Held {
    /// The client's own buffer: the last clone dropped releases it.
    Client(Buffer),
    /// A staging slot: busy while a clone lives.
    Staged(Arc<()>),
}

impl PartialEq for Held {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Held::Client(a), Held::Client(b)) => **a == **b,
            (Held::Staged(a), Held::Staged(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// One layer of a view, before the buffer ids and fds are filled in.
pub struct LayerSpec {
    /// Stable for the surface's life, so a layer keeps its plane while the
    /// tree around it changes.
    pub layer_id: u32,
    /// Held until the shell is done with it.
    pub held: Held,
    /// What its buffer id is keyed by.
    pub key: BufferKey,
    pub content: Content,
    /// The part of the buffer shown, in its own pixels, before `transform`.
    pub src: Rectangle<f64, smithay::utils::Buffer>,
    /// Where it lands, in view-local pixels.
    pub dst: Rectangle<i32, Logical>,
    /// wl_output.transform values, which IhsTransform shares.
    pub transform: u32,
    pub opaque: bool,
    /// The surface's content generation, to match presentation feedback.
    pub generation: u64,
}

/// A surface's layer id, kept in its data map.
struct LayerId(u32);

pub(crate) fn layer_id(states: &compositor::SurfaceData) -> u32 {
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

/// @p t applied to a buffer stored upside down: the vertical flip
/// (FLIPPED_180) first, then @p t.
fn after_y_flip(t: Transform) -> Transform {
    match t {
        Transform::Normal => Transform::Flipped180,
        Transform::_90 => Transform::Flipped270,
        Transform::_180 => Transform::Flipped,
        Transform::_270 => Transform::Flipped90,
        Transform::Flipped => Transform::_180,
        Transform::Flipped90 => Transform::_270,
        Transform::Flipped180 => Transform::Normal,
        Transform::Flipped270 => Transform::_90,
    }
}

/// The toplevel's window geometry origin: where its content starts within
/// the surface, so client-side shadows and the like fall outside the view.
pub(crate) fn geometry_origin(root: &WlSurface) -> Point<i32, Logical> {
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

/// The layers, how many surfaces had a buffer that could not be shown (a
/// shared-memory format other than ARGB/XRGB8888, or nothing to stage it
/// with), and the staging slots dropped, whose buffer ids are to retire.
pub struct Built {
    pub layers: Vec<LayerSpec>,
    pub skipped: usize,
    pub retired: Vec<u64>,
}

/// The layers of a view's scene, bottom to top.
pub fn build(
    trees: &[crate::popups::Tree],
    stager: &mut Stager,
    egl: &mut crate::egl_display::EglBuffers,
) -> Built {
    let mut built = Built {
        layers: Vec::new(),
        skipped: 0,
        retired: Vec::new(),
    };
    for (root, start) in trees {
        build_tree(root, *start, stager, egl, &mut built);
    }
    built
}

/// Add the layers of the tree at @p root, its origin at @p start.
fn build_tree(
    root: &WlSurface,
    start: Point<i32, Logical>,
    stager: &mut Stager,
    egl: &mut crate::egl_display::EglBuffers,
    built: &mut Built,
) {
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
            let (content, held, key) = match get_dmabuf(buffer) {
                Ok(dmabuf) => (
                    Content::Dmabuf(dmabuf.clone()),
                    Held::Client(buffer.clone()),
                    BufferKey::client(buffer),
                ),
                Err(_) => match egl.content(buffer) {
                    Some(content) => (
                        content,
                        Held::Client(buffer.clone()),
                        BufferKey::client(buffer),
                    ),
                    None => match staging::stage(stager, states, buffer, &data, &mut built.retired)
                    {
                        Some(s) => (
                            Content::Dmabuf(s.dmabuf),
                            Held::Staged(s.hold),
                            BufferKey::Staged(s.uid),
                        ),
                        None => {
                            built.skipped += 1;
                            return;
                        }
                    },
                },
            };
            let mut transform = data.buffer_transform();
            let mut src =
                view.src
                    .to_buffer(data.buffer_scale() as f64, transform, &buffer_size.to_f64());
            let (y_inverted, height) = match &content {
                Content::Dmabuf(d) => (d.y_inverted(), d.size().h as f64),
                Content::Image(i) => (i.y_inverted, i.height as f64),
            };
            if y_inverted {
                // Stored bottom row first: the part shown is mirrored in the
                // buffer, and the flip is undone on the way to the view.
                src.loc.y = height - (src.loc.y + src.size.h);
                transform = after_y_flip(transform);
            }
            let dst = Rectangle::new(*location + view.offset, view.dst);
            // Opaque when the client says every pixel of the surface is: its
            // opaque region covers the whole of it.
            let whole = Rectangle::from_size(view.dst);
            let opaque = data
                .opaque_regions()
                .is_some_and(|r| r.iter().any(|o| o.contains_rect(whole)));
            built.layers.push(LayerSpec {
                layer_id: layer_id(states),
                held,
                key,
                content,
                src,
                dst,
                transform: transform_value(transform),
                opaque,
                generation: crate::timing::generation(states),
            });
        },
        |_, _, _| true,
    );
}

/// The size of a dma-buf, for the frame description.
pub fn dmabuf_size(dmabuf: &Dmabuf) -> (u32, u32) {
    let size = dmabuf.size();
    (size.w.max(0) as u32, size.h.max(0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where transform @p t sends buffer point (x, y) of a w x h buffer.
    fn apply(t: Transform, (x, y): (i32, i32), (w, h): (i32, i32)) -> (i32, i32) {
        // Flip first (mirror across the vertical axis), then rotate
        // counter-clockwise, per wl_output.transform.
        let flipped = matches!(
            t,
            Transform::Flipped
                | Transform::Flipped90
                | Transform::Flipped180
                | Transform::Flipped270
        );
        let (mut x, y) = if flipped { (w - 1 - x, y) } else { (x, y) };
        let (mut y, mut w, mut h) = (y, w, h);
        let turns = match t {
            Transform::Normal | Transform::Flipped => 0,
            Transform::_90 | Transform::Flipped90 => 1,
            Transform::_180 | Transform::Flipped180 => 2,
            Transform::_270 | Transform::Flipped270 => 3,
        };
        for _ in 0..turns {
            (x, y) = (y, w - 1 - x);
            (w, h) = (h, w);
        }
        (x, y)
    }

    #[test]
    fn a_y_flip_composes_under_every_transform() {
        let size = (4, 3);
        for t in [
            Transform::Normal,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ] {
            for x in 0..size.0 {
                for y in 0..size.1 {
                    let unflipped = (x, size.1 - 1 - y);
                    assert_eq!(
                        apply(after_y_flip(t), (x, y), size),
                        apply(t, unflipped, size),
                        "{t:?} at {x},{y}"
                    );
                }
            }
        }
    }
}
