// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Popups show above their toplevel in its view, kept inside it; input
//! reaches them; they go away with a press outside the client, with
//! keyboard focus, or when destroyed.

mod common;

use common::client::{Input, Placement};
use common::harness::Harness;
use common::{mock_host, serial};
use ihs_wl_server::{ihs_wl_focus, ihs_wl_pointer, IhsWlPointerEvent, IhsWlPointerKind};

const APP: &str = "org.example.popup";
const BTN_LEFT: u32 = 0x110;
/// xdg_positioner.constraint_adjustment slide_x.
const SLIDE_X: u32 = 1;

fn pointer(view: i32, kind: IhsWlPointerKind, x: f64, y: f64, pressed: Option<bool>) {
    let ev = IhsWlPointerEvent {
        struct_size: std::mem::size_of::<IhsWlPointerEvent>(),
        kind: kind as u32,
        button: BTN_LEFT,
        x,
        y,
        pressed: pressed.unwrap_or(false) as u32,
        axis_source: 0,
        axis_x: 0.0,
        axis_y: 0.0,
        value120_x: 0,
        value120_y: 0,
        time_us: 1_000,
    };
    assert_eq!(unsafe { ihs_wl_pointer(view, &ev) }, 0);
}

/// A 200x100 toplevel shown by view 1, 640x480.
fn shown(socket: &str) -> Harness {
    let mut h = Harness::new(socket);
    h.client.use_seat();
    h.client.create_toplevel(APP, "popup");
    let buf = h.client.new_dmabuf(200, 100);
    h.client.commit_buffer(buf, false);
    h.bind(1, APP, 640.0, 480.0);
    h.wait_submissions(1, 1);
    h
}

fn menu(x: i32, y: i32) -> Placement {
    Placement {
        anchor: (x, y, 1, 1),
        size: (50, 40),
        adjust: SLIDE_X,
    }
}

/// The last submission with @p n layers.
fn layers(h: &Harness, n: usize) -> Vec<common::mock_host::SubmittedLayer> {
    h.rec.wait_for("layers", |_| {
        Harness::submissions(1)
            .last()
            .is_some_and(|s| s.layers.len() == n)
    });
    Harness::submissions(1).last().unwrap().layers.clone()
}

#[test]
fn a_popup_shows_above_its_toplevel_and_takes_input() {
    let _serial = serial();
    let mut h = shown("ihs-wl-test-popup");
    h.client.open_popup(menu(10, 20), false);
    assert_eq!(h.client.popup_geometry(), Some((10, 20, 50, 40)));

    let l = layers(&h, 2);
    assert_eq!(l[0].dst, (0, 0, 200, 100), "the toplevel, below");
    assert_eq!(l[1].dst, (10, 20, 50, 40), "the popup, above");

    let popup = h.client.popup_surface_id();
    pointer(1, IhsWlPointerKind::Motion, 15.0, 25.0, None);
    h.client.wait_input(
        "enter the popup",
        &[Input::Enter {
            surface: popup,
            x: 5.0,
            y: 5.0,
        }],
    );

    // Gone with the popup.
    h.client.close_popup();
    layers(&h, 1);
    mock_host::dispose_view(1);
}

#[test]
fn a_popup_is_kept_inside_its_view() {
    let _serial = serial();
    let mut h = shown("ihs-wl-test-popup-constrain");
    // Would run 10 px past the view's right edge; slides back in.
    h.client.open_popup(menu(600, 20), false);
    assert_eq!(h.client.popup_geometry(), Some((590, 20, 50, 40)));
    mock_host::dispose_view(1);
}

#[test]
fn a_press_outside_the_client_dismisses_a_grabbing_popup() {
    let _serial = serial();
    let mut h = shown("ihs-wl-test-popup-grab");
    // Open on a press, as a menu does.
    pointer(1, IhsWlPointerKind::Button, 30.0, 30.0, Some(true));
    h.client.wait_input(
        "press",
        &[Input::Button {
            button: BTN_LEFT,
            pressed: true,
        }],
    );
    h.client.open_popup(menu(30, 30), true);
    pointer(1, IhsWlPointerKind::Button, 30.0, 30.0, Some(false));

    // Nothing of the client's at 400,400.
    pointer(1, IhsWlPointerKind::Button, 400.0, 400.0, Some(true));
    h.client.dispatch_until("popup_done", |c| c.popup_done());
    mock_host::dispose_view(1);
}

#[test]
fn losing_focus_dismisses_popups() {
    let _serial = serial();
    let mut h = shown("ihs-wl-test-popup-focus");
    assert_eq!(ihs_wl_focus(1, 1), 0);
    h.client.open_popup(menu(10, 20), false);
    assert!(!h.client.popup_done());
    assert_eq!(ihs_wl_focus(1, 0), 0);
    h.client.dispatch_until("popup_done", |c| c.popup_done());
    mock_host::dispose_view(1);
}

#[test]
fn leaving_the_scene_dismisses_popups() {
    let _serial = serial();
    let mut h = shown("ihs-wl-test-popup-suspend");
    h.client.open_popup(menu(10, 20), false);
    mock_host::set_suspended(1, true);
    h.client.dispatch_until("popup_done", |c| c.popup_done());
    mock_host::dispose_view(1);
}
