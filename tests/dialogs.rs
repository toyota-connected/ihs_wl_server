// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Dialogs -- toplevels with a parent -- are shown in their parent's view:
//! centered over it and above it, taking the pointer over them and the
//! keyboard while they are up, and never bound to a view of their own.

mod common;

use std::time::{Duration, Instant};

use common::client::Input;
use common::harness::{params, Harness};
use common::{mock_host, serial};
use ihs_wl_server::{ihs_wl_focus, ihs_wl_pointer, IhsWlPointerEvent, IhsWlPointerKind};
use wayland_client::Proxy;
use wayland_protocols::xdg::shell::client::xdg_toplevel::State as S;

const APP: &str = "org.example.dialogs";

fn motion(view: i32, x: f64, y: f64) {
    let ev = IhsWlPointerEvent {
        struct_size: std::mem::size_of::<IhsWlPointerEvent>(),
        kind: IhsWlPointerKind::Motion as u32,
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
    assert!(unsafe { ihs_wl_pointer(view, &ev) } >= 0);
}

/// View 1's newest layers, once @p ok accepts them.
fn wait_layers(what: &str, ok: impl Fn(&[mock_host::SubmittedLayer]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(s) = Harness::submissions(1).last() {
            if ok(&s.layers) {
                return;
            }
        }
        assert!(Instant::now() < deadline, "no submission: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_dialog_is_shown_over_its_parent() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-dialogs");
    h.client.use_seat();
    h.client.create_toplevel(APP, "main");
    let main = h.client.new_dmabuf(200, 100);
    h.client.commit_buffer(main, false);
    h.bind(1, APP, 400.0, 300.0);
    assert_eq!(ihs_wl_focus(1, 1), 0);
    h.client.dispatch_until("main activated", |c| c.activated());

    // A 100x50 dialog: centered over the 200x100 window, above it.
    let buf = h.client.new_dmabuf(100, 50);
    let dialog = h.client.create_dialog("About", buf);
    wait_layers("dialog layer", |l| {
        l.len() == 2 && l[0].dst == (0, 0, 200, 100) && l[1].dst == (50, 25, 100, 50)
    });

    // It is the active window; its parent is not.
    h.client.dispatch_until("dialog activated", |c| {
        c.toplevel_has_state(&dialog.toplevel, S::Activated) && !c.activated()
    });

    // The pointer over it reaches it; beside it, the parent.
    let main_id = h.client.surface_id(false);
    let dialog_id = dialog.surface.id().protocol_id();
    motion(1, 60.0, 30.0);
    h.client.wait_input(
        "enter the dialog",
        &[
            Input::Enter {
                surface: dialog_id,
                x: 10.0,
                y: 5.0,
            },
            Input::Frame,
        ],
    );
    motion(1, 10.0, 10.0);
    h.client.wait_input(
        "enter the parent",
        &[
            Input::Leave { surface: dialog_id },
            Input::Enter {
                surface: main_id,
                x: 10.0,
                y: 10.0,
            },
            Input::Frame,
        ],
    );

    // Another view for the app does not take the dialog.
    assert_eq!(
        mock_host::create_view_with_params("ihs_wl/toplevel", 2, 400.0, 300.0, &params(APP)),
        0
    );
    h.client.roundtrip();
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        Harness::submissions(2).is_empty(),
        "the dialog bound a view"
    );

    // Closed, it goes, and the parent is the active window again.
    h.client.destroy_dialog(dialog);
    wait_layers("dialog gone", |l| l.len() == 1);
    h.client
        .dispatch_until("main activated again", |c| c.activated());
    mock_host::dispose_view(2);
    mock_host::dispose_view(1);
}
