// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! A view that asks its client for a size of its own: the toplevel is
//! configured to it, its content is scaled to fit the view (aspect kept,
//! centered) and moves with the view without a commit, and input is mapped
//! back into the client's space.

mod common;

use common::client::Input;
use common::harness::Harness;
use common::{mock_host, serial};
use ihs_wl_server::{ihs_wl_pointer, ihs_wl_view_size, IhsWlPointerEvent, IhsWlPointerKind};

const APP: &str = "org.example.requested";

/// creationParams for `{'app_id': app_id, 'requested_width': w,
/// 'requested_height': h}`. Each float64 is aligned to 8 bytes from the
/// start of the message.
fn params_requested(app_id: &str, w: f64, h: f64) -> Vec<u8> {
    let mut b = vec![13u8, 3]; // map, 3 entries
    let string = |b: &mut Vec<u8>, s: &str| {
        b.push(7);
        b.push(s.len() as u8);
        b.extend_from_slice(s.as_bytes());
    };
    let double = |b: &mut Vec<u8>, v: f64| {
        b.push(6);
        while !b.len().is_multiple_of(8) {
            b.push(0);
        }
        b.extend_from_slice(&v.to_le_bytes());
    };
    string(&mut b, "app_id");
    string(&mut b, app_id);
    string(&mut b, "requested_width");
    double(&mut b, w);
    string(&mut b, "requested_height");
    double(&mut b, h);
    b
}

/// The layers of view 1's newest submission once @p ok accepts them.
fn wait_layers(what: &str, ok: impl Fn(&[mock_host::SubmittedLayer]) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(s) = Harness::submissions(1).last() {
            if ok(&s.layers) {
                return;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no submission: {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn the_client_is_configured_to_the_requested_size() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-requested-configure");
    h.client.create_toplevel(APP, "requested");
    h.bind_with(1, &params_requested(APP, 800.0, 600.0), 400.0, 400.0);
    h.client
        .dispatch_until("configure to the requested size", |c| {
            c.configure_size() == Some((800, 600))
        });

    // A layout change does not move the client off it.
    mock_host::resize_view(1, 500.0, 300.0);
    assert_eq!(ihs_wl_view_size(1, 320.0, 240.0), 0);
    h.client
        .dispatch_until("configure to the new request", |c| {
            c.configure_size() == Some((320, 240))
        });

    // 0 x 0 goes back to the view's size.
    assert_eq!(ihs_wl_view_size(1, 0.0, 0.0), 0);
    h.client.dispatch_until("configure back to the view", |c| {
        c.configure_size() == Some((500, 300))
    });

    // One side only, or not finite, is refused.
    assert!(ihs_wl_view_size(1, 100.0, 0.0) < 0);
    assert!(ihs_wl_view_size(1, f64::NAN, 10.0) < 0);
    assert!(ihs_wl_view_size(1, -1.0, 10.0) < 0);
    mock_host::dispose_view(1);
}

#[test]
fn the_content_is_scaled_to_fit_and_follows_the_view() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-requested-fit");
    h.client.create_toplevel(APP, "requested");
    h.bind_with(1, &params_requested(APP, 800.0, 600.0), 400.0, 400.0);
    let buf = h.client.new_dmabuf(800, 600);
    h.client.commit_buffer(buf, false);

    // 800x600 into 400x400: half size, centered vertically.
    wait_layers("letterboxed", |l| {
        l.first().is_some_and(|l| l.dst == (0, 50, 400, 300))
    });

    // Wider than the content's aspect: bars at the sides now, with no commit
    // from the client. Scale 2/3: 533.3 wide from x = 133.3.
    let before = Harness::submissions(1).len();
    mock_host::resize_view(1, 800.0, 400.0);
    wait_layers("pillarboxed", |l| {
        l.first().is_some_and(|l| l.dst == (133, 0, 534, 400))
    });
    assert!(Harness::submissions(1).len() > before);
    mock_host::dispose_view(1);
}

#[test]
fn input_is_mapped_into_the_client() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-requested-input");
    h.client.use_seat();
    h.client.create_toplevel(APP, "requested");
    h.bind_with(1, &params_requested(APP, 800.0, 600.0), 400.0, 400.0);
    let buf = h.client.new_dmabuf(800, 600);
    h.client.commit_buffer(buf, false);
    h.wait_submissions(1, 1);
    let sid = h.client.surface_id(false);

    // View (200, 200) is client (400, 300): half scale, 50 down.
    let ev = IhsWlPointerEvent {
        struct_size: std::mem::size_of::<IhsWlPointerEvent>(),
        kind: IhsWlPointerKind::Motion as u32,
        button: 0,
        x: 200.0,
        y: 200.0,
        pressed: 0,
        axis_source: 0,
        axis_x: 0.0,
        axis_y: 0.0,
        value120_x: 0,
        value120_y: 0,
        time_us: 1_000,
    };
    assert!(unsafe { ihs_wl_pointer(1, &ev) } >= 0);
    h.client.wait_input(
        "enter at the mapped point",
        &[
            Input::Enter {
                surface: sid,
                x: 400.0,
                y: 300.0,
            },
            Input::Frame,
        ],
    );

    // In the bar above the content: nothing under it.
    let ev = IhsWlPointerEvent { y: 20.0, ..ev };
    assert!(unsafe { ihs_wl_pointer(1, &ev) } >= 0);
    h.client.wait_input(
        "leave in the bar",
        &[Input::Leave { surface: sid }, Input::Frame],
    );
    mock_host::dispose_view(1);
}

/// A client that commits a window larger than it was asked for (a minimum
/// size wider than the view) is scaled down to fit, not cut off.
#[test]
fn a_window_larger_than_asked_is_scaled_to_fit() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-oversized");
    h.client.use_seat();
    h.client.create_toplevel(APP, "oversized");
    h.bind(1, APP, 400.0, 400.0);
    h.client.dispatch_until("configured to the view", |c| {
        c.configure_size() == Some((400, 400))
    });
    // Asked for 400x400, it draws 500x400 anyway.
    let buf = h.client.new_dmabuf(500, 400);
    h.client.set_window_geometry(0, 0, 500, 400);
    h.client.commit_buffer(buf, false);

    // 500x400 into 400x400: 0.8, centered vertically.
    wait_layers("scaled down", |l| {
        l.first().is_some_and(|l| l.dst == (0, 40, 400, 320))
    });

    // Its right edge is still reachable: view (390, 200) is client
    // (487.5, 200).
    let sid = h.client.surface_id(false);
    let ev = IhsWlPointerEvent {
        struct_size: std::mem::size_of::<IhsWlPointerEvent>(),
        kind: IhsWlPointerKind::Motion as u32,
        button: 0,
        x: 390.0,
        y: 200.0,
        pressed: 0,
        axis_source: 0,
        axis_x: 0.0,
        axis_y: 0.0,
        value120_x: 0,
        value120_y: 0,
        time_us: 1_000,
    };
    assert!(unsafe { ihs_wl_pointer(1, &ev) } >= 0);
    h.client.wait_input(
        "enter at the mapped point",
        &[
            Input::Enter {
                surface: sid,
                x: 487.5,
                y: 200.0,
            },
            Input::Frame,
        ],
    );
    mock_host::dispose_view(1);
}
