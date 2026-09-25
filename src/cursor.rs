// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! The cursor the client under the pointer asks for, for the shell to show.
//!
//! The module draws nothing, so the cursor is the Flutter view's: each
//! pointer event returns the shape the client last asked for, and the
//! widget sets that as its mouse cursor. A client asks through
//! wp_cursor_shape_v1 (a named shape) or wl_pointer.set_cursor (a surface
//! of its own, shown as the default here, or none, which hides it). The
//! answer is read without waiting on the compositor thread, so it can lag
//! the event it comes back from by one.

use std::sync::atomic::{AtomicI32, Ordering};

use smithay::input::pointer::{CursorIcon, CursorImageStatus};
use smithay::reexports::wayland_protocols::wp::cursor_shape::v1::server::wp_cursor_shape_device_v1::Shape;

/// No shape asked for yet.
pub(crate) const NONE: i32 = 0;
/// The client hid the cursor.
pub(crate) const HIDDEN: i32 = crate::IHS_WL_CURSOR_HIDDEN;

static CURRENT: AtomicI32 = AtomicI32::new(NONE);

/// What `ihs_wl_pointer` returns.
pub fn current() -> i32 {
    CURRENT.load(Ordering::Relaxed)
}

/// A server starting: no client has asked for anything.
pub fn reset() {
    CURRENT.store(NONE, Ordering::Relaxed);
}

/// The client under the pointer asked for @p status. Smithay also sets the
/// default when the pointer leaves a surface.
pub fn set(status: &CursorImageStatus) {
    let value = match status {
        CursorImageStatus::Hidden => HIDDEN,
        CursorImageStatus::Named(icon) => shape_of(*icon) as i32,
        // A surface of the client's own: nothing draws it, so the default.
        CursorImageStatus::Surface(_) => Shape::Default as i32,
    };
    CURRENT.store(value, Ordering::Relaxed);
}

/// The wp_cursor_shape_device_v1.shape for @p icon.
fn shape_of(icon: CursorIcon) -> Shape {
    match icon {
        CursorIcon::ContextMenu => Shape::ContextMenu,
        CursorIcon::Help => Shape::Help,
        CursorIcon::Pointer => Shape::Pointer,
        CursorIcon::Progress => Shape::Progress,
        CursorIcon::Wait => Shape::Wait,
        CursorIcon::Cell => Shape::Cell,
        CursorIcon::Crosshair => Shape::Crosshair,
        CursorIcon::Text => Shape::Text,
        CursorIcon::VerticalText => Shape::VerticalText,
        CursorIcon::Alias => Shape::Alias,
        CursorIcon::Copy => Shape::Copy,
        CursorIcon::Move => Shape::Move,
        CursorIcon::NoDrop => Shape::NoDrop,
        CursorIcon::NotAllowed => Shape::NotAllowed,
        CursorIcon::Grab => Shape::Grab,
        CursorIcon::Grabbing => Shape::Grabbing,
        CursorIcon::EResize => Shape::EResize,
        CursorIcon::NResize => Shape::NResize,
        CursorIcon::NeResize => Shape::NeResize,
        CursorIcon::NwResize => Shape::NwResize,
        CursorIcon::SResize => Shape::SResize,
        CursorIcon::SeResize => Shape::SeResize,
        CursorIcon::SwResize => Shape::SwResize,
        CursorIcon::WResize => Shape::WResize,
        CursorIcon::EwResize => Shape::EwResize,
        CursorIcon::NsResize => Shape::NsResize,
        CursorIcon::NeswResize => Shape::NeswResize,
        CursorIcon::NwseResize => Shape::NwseResize,
        CursorIcon::ColResize => Shape::ColResize,
        CursorIcon::RowResize => Shape::RowResize,
        CursorIcon::AllScroll => Shape::AllScroll,
        CursorIcon::ZoomIn => Shape::ZoomIn,
        CursorIcon::ZoomOut => Shape::ZoomOut,
        CursorIcon::DndAsk => Shape::DndAsk,
        CursorIcon::AllResize => Shape::AllResize,
        _ => Shape::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_keep_their_protocol_numbers() {
        assert_eq!(shape_of(CursorIcon::Default) as i32, 1);
        assert_eq!(shape_of(CursorIcon::Pointer) as i32, 4);
        assert_eq!(shape_of(CursorIcon::Text) as i32, 9);
        assert_eq!(shape_of(CursorIcon::AllResize) as i32, 36);
    }
}
