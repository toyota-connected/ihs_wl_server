// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! The cursor the client under the pointer asks for comes back from
//! ihs_wl_pointer: a named shape, the default for a cursor surface of its
//! own, hidden, and the default again once the pointer leaves.

mod common;

use common::client::Input;
use common::harness::Harness;
use common::{mock_host, serial};
use ihs_wl_server::{ihs_wl_pointer, IhsWlPointerEvent, IhsWlPointerKind, IHS_WL_CURSOR_HIDDEN};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;

const APP: &str = "org.example.cursor";

/// Send a pointer event to view 1; the cursor as of the events before it.
fn pointer(kind: IhsWlPointerKind, x: f64, y: f64) -> i32 {
    let ev = IhsWlPointerEvent {
        struct_size: std::mem::size_of::<IhsWlPointerEvent>(),
        kind: kind as u32,
        button: 0,
        x,
        y,
        pressed: 0,
        axis_source: 0,
        axis_x: 0.0,
        axis_y: 0.0,
        value120_x: 0,
        value120_y: 0,
        time_us: 1_000,
    };
    unsafe { ihs_wl_pointer(1, &ev) }
}

#[test]
fn pointer_returns_the_cursor_the_client_asks_for() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-cursor");
    assert!(h
        .client
        .globals()
        .iter()
        .any(|g| g == "wp_cursor_shape_manager_v1"));
    h.client.use_seat();
    h.client.create_toplevel(APP, "cursor");
    let buf = h.client.new_dmabuf(100, 80);
    h.client.commit_buffer(buf, false);
    h.bind(1, APP, 640.0, 480.0);
    h.wait_submissions(1, 1);
    let sid = h.client.surface_id(false);

    assert_eq!(
        pointer(IhsWlPointerKind::Motion, 10.0, 10.0),
        0,
        "none asked"
    );
    h.client.wait_input(
        "enter",
        &[Input::Enter {
            surface: sid,
            x: 10.0,
            y: 10.0,
        }],
    );

    h.client.set_cursor_shape(Shape::Text);
    assert_eq!(
        pointer(IhsWlPointerKind::Motion, 11.0, 10.0),
        Shape::Text as i32
    );
    h.client.set_cursor_shape(Shape::NwseResize);
    assert_eq!(
        pointer(IhsWlPointerKind::Motion, 12.0, 10.0),
        Shape::NwseResize as i32
    );

    h.client.set_cursor_surface(false);
    assert_eq!(
        pointer(IhsWlPointerKind::Motion, 13.0, 10.0),
        IHS_WL_CURSOR_HIDDEN
    );
    h.client.set_cursor_surface(true);
    assert_eq!(
        pointer(IhsWlPointerKind::Motion, 14.0, 10.0),
        Shape::Default as i32
    );

    h.client.set_cursor_shape(Shape::Pointer);
    pointer(IhsWlPointerKind::Leave, 0.0, 0.0);
    h.client
        .wait_input("leave", &[Input::Leave { surface: sid }]);
    assert_eq!(
        pointer(IhsWlPointerKind::Motion, 700.0, 10.0),
        Shape::Default as i32
    );
    mock_host::dispose_view(1);
}
