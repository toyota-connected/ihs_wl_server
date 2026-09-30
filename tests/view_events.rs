// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! View events posted to Dart's port: a view binds, its client asks to
//! change window state, and it closes -- on its own or when the view asks
//! it to. The poster stands in for `NativeApi.postCObject` and reads each
//! message as a `Dart_CObject` list `[event, view_id]`, as Dart would.

mod common;

use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::harness::Harness;
use common::{mock_host, serial};
use ihs_wl_server::{ihs_wl_set_event_port, ihs_wl_view_close, IhsWlViewEvent as E};
use wayland_protocols::xdg::shell::client::xdg_toplevel;

const APP: &str = "org.example.events";
const PORT: i64 = 4711;

static POSTED: Mutex<Vec<(i64, i64, i64)>> = Mutex::new(Vec::new());

/// Read a `Dart_CObject` `[int64, int64]` and record it with its port.
unsafe extern "C" fn record(port: i64, message: *mut c_void) -> bool {
    let word = std::mem::size_of::<usize>();
    let base = message as *const u8;
    assert_eq!(*(base as *const i32), 6, "not an array");
    let length = *(base.add(8) as *const isize);
    assert_eq!(length, 2);
    let values = *(base.add(8 + word) as *const *const *const u8);
    let int64 = |i: usize| {
        let item = *values.add(i);
        assert_eq!(*(item as *const i32), 3, "not an int64");
        *(item.add(8) as *const i64)
    };
    POSTED.lock().unwrap().push((port, int64(0), int64(1)));
    true
}

fn wait_event(what: &str, event: E, view: i32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if POSTED
            .lock()
            .unwrap()
            .contains(&(PORT, event as i64, view as i64))
        {
            return;
        }
        assert!(Instant::now() < deadline, "no event: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_view_reports_binding_window_requests_and_closing() {
    let _serial = serial();
    POSTED.lock().unwrap().clear();
    let mut h = Harness::new("ihs-wl-test-view-events");
    assert_eq!(ihs_wl_set_event_port(Some(record), PORT), 0);
    h.client.create_toplevel(APP, "events");
    let buf = h.client.new_dmabuf(64, 64);
    h.client.commit_buffer(buf, false);
    h.bind(1, APP, 64.0, 64.0);
    wait_event("bound", E::Bound, 1);

    // Window-state requests reach the app; the toplevel stays maximized to
    // the view.
    let mut ask = |event: E, send: fn(&xdg_toplevel::XdgToplevel)| {
        send(h.client.toplevel());
        h.client.roundtrip();
        wait_event("window request", event, 1);
    };
    ask(E::MaximizeRequested, |t| t.set_maximized());
    ask(E::UnmaximizeRequested, |t| t.unset_maximized());
    ask(E::MinimizeRequested, |t| t.set_minimized());
    ask(E::FullscreenRequested, |t| t.set_fullscreen(None));
    ask(E::UnfullscreenRequested, |t| t.unset_fullscreen());
    assert_eq!(h.client.configure_size(), Some((64, 64)));

    // Asked to close, the client does.
    assert_eq!(ihs_wl_view_close(1), 0);
    h.client
        .dispatch_until("close request", |c| c.close_requests() == 1);
    h.client.destroy_toplevel();
    wait_event("closed", E::Closed, 1);

    // A view showing nothing is left alone.
    assert_eq!(ihs_wl_view_close(1), 0);
    h.client.roundtrip();
    assert_eq!(h.client.close_requests(), 1);

    // Stop posting.
    assert_eq!(ihs_wl_set_event_port(None, 0), 0);
    mock_host::dispose_view(1);
}
