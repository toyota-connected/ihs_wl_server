// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Input from the Dart controller reaches the client under it: pointer
//! enter/motion/buttons/scroll/leave, touch, and keys while its view has
//! focus. Coordinates are view-local, offset by the window geometry.

mod common;

use common::client::Input;
use common::harness::Harness;
use common::{mock_host, serial};
use ihs_wl_server::{
    ihs_wl_focus, ihs_wl_key, ihs_wl_pointer, ihs_wl_touch, IhsWlPointerEvent, IhsWlPointerKind,
    IhsWlResult, IhsWlTouchEvent, IhsWlTouchKind,
};

const BTN_LEFT: u32 = 0x110;
const KEY_A: u32 = 30;
const KEY_LEFTSHIFT: u32 = 42;
const APP: &str = "org.example.input";

fn pointer_event(kind: IhsWlPointerKind, x: f64, y: f64) -> IhsWlPointerEvent {
    IhsWlPointerEvent {
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
    }
}

fn pointer(view: i32, ev: IhsWlPointerEvent) {
    assert_eq!(unsafe { ihs_wl_pointer(view, &ev) }, 0);
}

fn motion(view: i32, x: f64, y: f64) {
    pointer(view, pointer_event(IhsWlPointerKind::Motion, x, y));
}

fn button(view: i32, x: f64, y: f64, pressed: bool) {
    let mut ev = pointer_event(IhsWlPointerKind::Button, x, y);
    ev.button = BTN_LEFT;
    ev.pressed = pressed as u32;
    pointer(view, ev);
}

fn touch(view: i32, kind: IhsWlTouchKind, slot: i32, x: f64, y: f64) {
    let ev = IhsWlTouchEvent {
        struct_size: std::mem::size_of::<IhsWlTouchEvent>(),
        kind: kind as u32,
        slot,
        x,
        y,
        time_us: 1_000,
    };
    assert_eq!(unsafe { ihs_wl_touch(view, &ev) }, 0);
}

/// A 100x80 toplevel shown by view 1, the client holding the seat's devices.
fn shown(socket: &str) -> Harness {
    let mut h = Harness::new(socket);
    h.client.use_seat();
    h.client.create_toplevel(APP, "input");
    let buf = h.client.new_dmabuf(100, 80);
    h.client.commit_buffer(buf, false);
    h.bind(1, APP, 640.0, 480.0);
    h.wait_submissions(1, 1);
    h
}

#[test]
fn pointer_enters_moves_clicks_scrolls_and_leaves() {
    let _serial = serial();
    let mut h = shown("ihs-wl-test-pointer");
    let sid = h.client.surface_id(false);

    motion(1, 10.5, 20.0);
    h.client.wait_input(
        "enter",
        &[
            Input::Enter {
                surface: sid,
                x: 10.5,
                y: 20.0,
            },
            Input::Frame,
        ],
    );
    motion(1, 30.0, 40.0);
    h.client.wait_input(
        "motion",
        &[Input::Motion { x: 30.0, y: 40.0 }, Input::Frame],
    );

    let mut ev = pointer_event(IhsWlPointerKind::Axis, 30.0, 40.0);
    ev.axis_y = 15.0;
    ev.value120_y = 120;
    pointer(1, ev);
    h.client.wait_input(
        "scroll",
        &[
            Input::AxisSource(0),
            Input::AxisValue120 {
                horizontal: false,
                value120: 120,
            },
            Input::Axis {
                horizontal: false,
                value: 15.0,
            },
            Input::Frame,
        ],
    );

    // Leaving mid-click keeps the surface until the release.
    button(1, 30.0, 40.0, true);
    pointer(1, pointer_event(IhsWlPointerKind::Leave, 0.0, 0.0));
    motion(1, 700.0, 40.0);
    let got = h.client.wait_input(
        "press, drag out",
        &[
            Input::Button {
                button: BTN_LEFT,
                pressed: true,
            },
            Input::Motion { x: 700.0, y: 40.0 },
        ],
    );
    assert!(
        !got.iter().any(|e| matches!(e, Input::Leave { .. })),
        "left mid-click: {got:?}"
    );
    button(1, 700.0, 40.0, false);
    h.client.wait_input(
        "release, then leave",
        &[
            Input::Button {
                button: BTN_LEFT,
                pressed: false,
            },
            Input::Leave { surface: sid },
        ],
    );
    mock_host::dispose_view(1);
}

#[test]
fn pointer_hits_the_surface_under_it_past_the_window_geometry() {
    let _serial = serial();
    let mut h = shown("ihs-wl-test-pointer-hit");
    // Content starts 10,10 into the surface; a subsurface sits at 50,40.
    h.client.set_window_geometry(10, 10, 80, 60);
    let sub = h.client.new_dmabuf(20, 20);
    h.client.add_subsurface(sub, 50, 40);
    let (main, sub) = (h.client.surface_id(false), h.client.surface_id(true));

    motion(1, 0.0, 0.0);
    h.client.wait_input(
        "enter the toplevel at its geometry origin",
        &[Input::Enter {
            surface: main,
            x: 10.0,
            y: 10.0,
        }],
    );
    motion(1, 45.0, 35.0);
    h.client.wait_input(
        "enter the subsurface",
        &[
            Input::Leave { surface: main },
            Input::Enter {
                surface: sub,
                x: 5.0,
                y: 5.0,
            },
        ],
    );
    // Past the toplevel's buffer: nothing under the pointer.
    motion(1, 200.0, 200.0);
    h.client
        .wait_input("leave", &[Input::Leave { surface: sub }, Input::Frame]);
    mock_host::dispose_view(1);
}

#[test]
fn touch_stays_with_the_surface_it_went_down_on() {
    let _serial = serial();
    let mut h = shown("ihs-wl-test-touch");
    let sid = h.client.surface_id(false);

    touch(1, IhsWlTouchKind::Down, 3, 12.0, 34.0);
    touch(1, IhsWlTouchKind::Frame, 0, 0.0, 0.0);
    touch(1, IhsWlTouchKind::Motion, 3, 200.0, 300.0);
    touch(1, IhsWlTouchKind::Frame, 0, 0.0, 0.0);
    touch(1, IhsWlTouchKind::Up, 3, 0.0, 0.0);
    touch(1, IhsWlTouchKind::Frame, 0, 0.0, 0.0);
    h.client.wait_input(
        "down, motion, up",
        &[
            Input::TouchDown {
                surface: sid,
                id: 3,
                x: 12.0,
                y: 34.0,
            },
            Input::TouchFrame,
            Input::TouchMotion {
                id: 3,
                x: 200.0,
                y: 300.0,
            },
            Input::TouchFrame,
            Input::TouchUp { id: 3 },
            Input::TouchFrame,
        ],
    );

    touch(1, IhsWlTouchKind::Down, 4, 1.0, 1.0);
    touch(1, IhsWlTouchKind::Cancel, 0, 0.0, 0.0);
    h.client.wait_input("cancel", &[Input::TouchCancel]);
    mock_host::dispose_view(1);
}

#[test]
fn keys_follow_the_focused_view() {
    let _serial = serial();
    let mut h = shown("ihs-wl-test-keys");
    let sid = h.client.surface_id(false);

    // Not focused yet: dropped.
    assert_eq!(ihs_wl_key(1, KEY_A, 1, 0), 0);
    assert_eq!(ihs_wl_key(1, KEY_A, 0, 0), 0);
    assert_eq!(ihs_wl_focus(1, 1), 0);
    let got = h.client.wait_input(
        "keymap, repeat info, enter",
        &[
            Input::Keymap,
            Input::RepeatInfo {
                rate: 25,
                delay: 600,
            },
            Input::KeyEnter { surface: sid },
        ],
    );
    assert!(
        !got.iter().any(|e| matches!(e, Input::Key { .. })),
        "a key reached an unfocused client: {got:?}"
    );

    assert_eq!(ihs_wl_key(1, KEY_A, 1, 0), 0);
    assert_eq!(ihs_wl_key(1, KEY_A, 0, 0), 0);
    h.client.wait_input(
        "key press and release",
        &[
            Input::Key {
                key: KEY_A,
                pressed: true,
            },
            Input::Key {
                key: KEY_A,
                pressed: false,
            },
        ],
    );

    // Out of the scene, the client loses focus; back in, it has it again.
    mock_host::set_suspended(1, true);
    h.client
        .wait_input("leave on suspend", &[Input::KeyLeave { surface: sid }]);
    mock_host::set_suspended(1, false);
    h.client
        .wait_input("enter on resume", &[Input::KeyEnter { surface: sid }]);

    // A key held as focus goes is forgotten: Shift stays down no longer.
    assert_eq!(ihs_wl_key(1, KEY_LEFTSHIFT, 1, 0), 0);
    h.client
        .wait_input("shift down", &[Input::Modifiers { depressed: 1 }]);
    assert_eq!(ihs_wl_focus(1, 0), 0);
    assert_eq!(ihs_wl_focus(1, 1), 0);
    h.client.wait_input(
        "enter with no modifiers",
        &[
            Input::KeyLeave { surface: sid },
            Input::KeyEnter { surface: sid },
            Input::Modifiers { depressed: 0 },
        ],
    );

    // Taking focus from a view that lacks it changes nothing.
    assert_eq!(ihs_wl_focus(2, 0), 0);
    assert_eq!(ihs_wl_focus(1, 0), 0);
    let got = h
        .client
        .wait_input("leave", &[Input::KeyLeave { surface: sid }]);
    assert_eq!(
        got.iter()
            .filter(|e| matches!(e, Input::KeyLeave { .. }))
            .count(),
        1,
        "{got:?}"
    );
    mock_host::dispose_view(1);
}

#[test]
fn malformed_input_is_rejected() {
    let _serial = serial();
    let _h = Harness::new("ihs-wl-test-input-bad");
    let invalid = IhsWlResult::ErrInvalid as i32;

    let mut ev = pointer_event(IhsWlPointerKind::Motion, 1.0, 1.0);
    ev.kind = 99;
    assert_eq!(unsafe { ihs_wl_pointer(1, &ev) }, invalid);
    let mut ev = pointer_event(IhsWlPointerKind::Motion, f64::NAN, 1.0);
    assert_eq!(unsafe { ihs_wl_pointer(1, &ev) }, invalid);
    ev.x = 1.0;
    ev.struct_size = 8;
    assert_eq!(unsafe { ihs_wl_pointer(1, &ev) }, invalid);
    assert_eq!(unsafe { ihs_wl_pointer(1, std::ptr::null()) }, invalid);

    let ev = IhsWlTouchEvent {
        struct_size: std::mem::size_of::<IhsWlTouchEvent>(),
        kind: IhsWlTouchKind::Down as u32,
        slot: -1,
        x: 0.0,
        y: 0.0,
        time_us: 0,
    };
    assert_eq!(unsafe { ihs_wl_touch(1, &ev) }, invalid);
    assert_eq!(ihs_wl_key(1, 0x300, 1, 0), invalid);
}
