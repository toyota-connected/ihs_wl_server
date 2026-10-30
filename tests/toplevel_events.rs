// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Window events posted to Dart's port: every window maps, changes and goes
//! whether or not a view shows it -- one another app launched, say -- and a
//! view naming a window's id binds it, whatever its app_id. Dialogs are
//! shown with their parent and never posted. A window activated with a
//! client's own token asks to be shown: activation when the focused client
//! made it for user input and still has focus, attention otherwise.

mod common;

use std::ffi::{c_void, CStr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::client::Client;
use common::harness::Harness;
use common::{mock_host, serial};
use ihs_wl_server::{
    ihs_wl_focus, ihs_wl_key, ihs_wl_pointer, ihs_wl_set_event_port, ihs_wl_toplevel_info,
    ihs_wl_toplevels, toplevel_info, toplevels, IhsWlPointerEvent, IhsWlPointerKind,
    IhsWlToplevelEvent as T, IhsWlToplevelInfo,
};

const APP: &str = "org.example.Foreign";
const PORT: i64 = 4712;

static POSTED: Mutex<Vec<(i64, i64)>> = Mutex::new(Vec::new());

/// Read a `Dart_CObject` `[int64, int64]`, as Dart would.
unsafe extern "C" fn record(port: i64, message: *mut c_void) -> bool {
    assert_eq!(port, PORT);
    let word = std::mem::size_of::<usize>();
    let base = message as *const u8;
    assert_eq!(*(base as *const i32), 6, "not an array");
    assert_eq!(*(base.add(8) as *const isize), 2);
    let values = *(base.add(8 + word) as *const *const *const u8);
    let int64 = |i: usize| {
        let item = *values.add(i);
        assert_eq!(*(item as *const i32), 3, "not an int64");
        *(item.add(8) as *const i64)
    };
    POSTED.lock().unwrap().push((int64(0), int64(1)));
    true
}

/// A key press and release on view 1, which the launcher's window gets.
fn press_key(h: &mut Harness) {
    const KEY_A: u32 = 30;
    assert_eq!(ihs_wl_key(1, KEY_A, 1, 0), 0);
    assert_eq!(ihs_wl_key(1, KEY_A, 0, 0), 0);
    h.client.roundtrip();
}

/// A left button press or release on view 1.
fn button(h: &mut Harness, pressed: bool) {
    let ev = IhsWlPointerEvent {
        struct_size: std::mem::size_of::<IhsWlPointerEvent>(),
        kind: IhsWlPointerKind::Button as u32,
        button: 0x110,
        x: 10.0,
        y: 10.0,
        pressed: pressed as u32,
        axis_source: 0,
        axis_x: 0.0,
        axis_y: 0.0,
        value120_x: 0,
        value120_y: 0,
        time_us: 1_000,
    };
    assert!(unsafe { ihs_wl_pointer(1, &ev) } >= 0);
    h.client.roundtrip();
}

/// Give anything that would be posted time to be.
fn settle(h: &mut Harness) {
    h.client.roundtrip();
    std::thread::sleep(Duration::from_millis(50));
}

/// How many times @p event was posted for window @p id.
fn count(event: T, id: i64) -> usize {
    POSTED
        .lock()
        .unwrap()
        .iter()
        .filter(|p| **p == (event as i64, id))
        .count()
}

/// Wait until @p event was posted @p n times for window @p id.
fn wait(what: &str, event: T, id: i64, n: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while count(event, id) < n {
        assert!(Instant::now() < deadline, "no event: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The windows posted as mapped so far.
fn mapped() -> Vec<i64> {
    POSTED
        .lock()
        .unwrap()
        .iter()
        .filter(|(e, _)| *e == T::Mapped as i64)
        .map(|(_, id)| *id)
        .collect()
}

/// creationParams for `{'app_id': app_id, 'toplevel_id': id}`.
fn params_toplevel(app_id: &str, id: i64) -> Vec<u8> {
    let mut b = vec![13u8, 2]; // map, 2 entries
    for s in ["app_id", app_id, "toplevel_id"] {
        b.push(7);
        b.push(s.len() as u8);
        b.extend_from_slice(s.as_bytes());
    }
    b.push(4); // int64
    b.extend_from_slice(&id.to_le_bytes());
    b
}

fn c_str(chars: &[std::ffi::c_char]) -> &str {
    unsafe { CStr::from_ptr(chars.as_ptr()) }.to_str().unwrap()
}

#[test]
fn windows_are_posted_and_a_view_binds_one_by_id() {
    let _serial = serial();
    POSTED.lock().unwrap().clear();
    let mut h = Harness::new("ihs-wl-test-toplevel-events");
    assert_eq!(ihs_wl_set_event_port(Some(record), PORT), 0);
    assert!(toplevels().unwrap().is_empty());

    // A window no view asked for.
    h.client.create_toplevel(APP, "first");
    let buf = h.client.new_dmabuf(64, 64);
    h.client.commit_buffer(buf, false);
    let deadline = Instant::now() + Duration::from_secs(5);
    while mapped().is_empty() {
        assert!(Instant::now() < deadline, "no event: mapped");
        std::thread::sleep(Duration::from_millis(5));
    }
    let id = mapped()[0];
    let info = toplevel_info(id).unwrap().expect("mapped window");
    assert_eq!((info.app_id.as_str(), info.title.as_str()), (APP, "first"));
    assert_eq!(info.view, None);
    assert_eq!(info.client.pid, std::process::id() as i32);
    assert_eq!(info.client.sandbox, None);
    assert_eq!(toplevels().unwrap(), vec![id]);

    // The same over the C ABI.
    let mut c: IhsWlToplevelInfo = unsafe { std::mem::zeroed() };
    c.struct_size = std::mem::size_of::<IhsWlToplevelInfo>();
    assert_eq!(unsafe { ihs_wl_toplevel_info(id, &mut c) }, 1);
    assert_eq!(c.view_id, -1);
    assert_eq!((c_str(&c.app_id), c_str(&c.title)), (APP, "first"));
    assert_eq!(c.pid, std::process::id() as i32);
    assert!(!c.sandboxed);
    let mut ids = [0i64; 4];
    assert_eq!(unsafe { ihs_wl_toplevels(ids.as_mut_ptr(), ids.len()) }, 1);
    assert_eq!(ids[0], id);
    // Asking how many there are.
    assert_eq!(unsafe { ihs_wl_toplevels(std::ptr::null_mut(), 0) }, 1);

    // A dialog is shown with its parent: not a window.
    let dialog = h.client.create_dialog("dialog", buf);
    assert_eq!(toplevels().unwrap(), vec![id]);
    assert_eq!(mapped(), vec![id]);

    // Its client renames it.
    h.client.toplevel().set_title("second".into());
    h.client.roundtrip();
    wait("renamed", T::Changed, id, 1);
    assert_eq!(toplevel_info(id).unwrap().unwrap().title, "second");

    // A view naming another window's id leaves it alone...
    assert_eq!(
        mock_host::create_view_with_params(
            "ihs_wl/toplevel",
            2,
            64.0,
            64.0,
            &params_toplevel(APP, id + 1000)
        ),
        0
    );
    h.client.roundtrip();
    assert_eq!(toplevel_info(id).unwrap().unwrap().view, None);
    // ...and one naming it binds it, though it names another app_id.
    h.bind_with(1, &params_toplevel("org.example.Other", id), 64.0, 64.0);
    wait("bound", T::Changed, id, 2);
    assert_eq!(toplevel_info(id).unwrap().unwrap().view, Some(1));

    // Gone: no longer a window.
    h.client.destroy_dialog(dialog);
    h.client.destroy_toplevel();
    wait("unmapped", T::Unmapped, id, 1);
    assert_eq!(toplevel_info(id).unwrap(), None);
    assert!(toplevels().unwrap().is_empty());
    assert_eq!(unsafe { ihs_wl_toplevel_info(id, &mut c) }, 0);
    assert_eq!(mapped(), vec![id], "the dialog was never a window");

    assert_eq!(ihs_wl_set_event_port(None, 0), 0);
    mock_host::dispose_view(1);
    mock_host::dispose_view(2);
}

/// Wait until @p n windows were posted as mapped; their ids.
fn wait_mapped(n: usize) -> Vec<i64> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while mapped().len() < n {
        assert!(Instant::now() < deadline, "no event: {n} mapped");
        std::thread::sleep(Duration::from_millis(5));
    }
    mapped()
}

#[test]
fn a_window_asking_to_be_shown_is_posted() {
    const SOCKET: &str = "ihs-wl-test-toplevel-activation";
    let _serial = serial();
    POSTED.lock().unwrap().clear();
    let mut h = Harness::new(SOCKET);
    assert_eq!(ihs_wl_set_event_port(Some(record), PORT), 0);

    // A launcher, shown and focused.
    h.client.create_toplevel("org.example.Launcher", "launcher");
    let buf = h.client.new_dmabuf(64, 64);
    h.client.commit_buffer(buf, false);
    h.bind(1, "org.example.Launcher", 64.0, 64.0);
    assert_eq!(ihs_wl_focus(1, 1), 0);
    h.client
        .dispatch_until("launcher activated", |c| c.activated());
    let launcher = wait_mapped(1)[0];

    // Another app's window, which no view shows.
    let mut app = Client::connect(SOCKET);
    app.create_toplevel(APP, "app");
    let buf = app.new_dmabuf(64, 64);
    app.commit_buffer(buf, false);
    let id = wait_mapped(2)[1];

    // With a token of its own: nothing says the user asked.
    let own = app.client_token();
    app.activate(&own);
    wait("attention", T::AttentionRequested, id, 1);
    assert_eq!(count(T::ActivationRequested, id), 0);

    // One the focused launcher made with no input behind it: attention,
    // which is not posted again within a second.
    let token = h.client.client_token();
    app.activate(&token);
    settle(&mut h);
    assert_eq!(count(T::ActivationRequested, id), 0);
    assert_eq!(count(T::AttentionRequested, id), 1);

    // One it made for a key press, as when the user taps it.
    press_key(&mut h);
    let token = h.client.input_token();
    app.activate(&token);
    wait("activation", T::ActivationRequested, id, 1);

    // A token is good once.
    app.activate(&token);
    settle(&mut h);
    assert_eq!(count(T::ActivationRequested, id), 1);
    assert_eq!(count(T::AttentionRequested, id), 1);

    // The active window asking: nothing to do.
    press_key(&mut h);
    let token = h.client.input_token();
    h.client.activate(&token);
    settle(&mut h);
    assert_eq!(count(T::ActivationRequested, launcher), 0);
    assert_eq!(count(T::AttentionRequested, launcher), 0);

    // Made while focused, used after the launcher lost focus: by the app,
    // and by the launcher for itself.
    press_key(&mut h);
    let later = h.client.input_token();
    let own_later = h.client.input_token();
    assert_eq!(ihs_wl_focus(1, 0), 0);
    std::thread::sleep(Duration::from_secs(1));
    app.activate(&later);
    wait("attention", T::AttentionRequested, id, 2);
    h.client.activate(&own_later);
    wait("attention", T::AttentionRequested, launcher, 1);
    assert_eq!(count(T::ActivationRequested, id), 1);
    assert_eq!(count(T::ActivationRequested, launcher), 0);

    // Activated before its first commit: posted once it maps.
    assert_eq!(ihs_wl_focus(1, 1), 0);
    h.client
        .dispatch_until("launcher activated", |c| c.activated());
    press_key(&mut h);
    let token = h.client.input_token();
    let mut late = Client::connect(SOCKET);
    late.create_toplevel_activated("org.example.Late", "late", &token);
    let late_id = wait_mapped(3)[2];
    wait("activation", T::ActivationRequested, late_id, 1);

    // Held until it maps, by when the launcher lost focus.
    press_key(&mut h);
    let token = h.client.input_token();
    assert_eq!(ihs_wl_focus(1, 0), 0);
    settle(&mut h);
    let mut held = Client::connect(SOCKET);
    held.create_toplevel_activated("org.example.Held", "held", &token);
    let held_id = wait_mapped(4)[3];
    wait("attention", T::AttentionRequested, held_id, 1);
    assert_eq!(count(T::ActivationRequested, held_id), 0);

    // A press on the unfocused launcher, which focuses it: the press is
    // its input.
    button(&mut h, true);
    assert_eq!(ihs_wl_focus(1, 1), 0);
    h.client
        .dispatch_until("launcher activated", |c| c.activated());
    let token = h.client.input_token();
    button(&mut h, false);
    app.activate(&token);
    wait("activation", T::ActivationRequested, id, 2);

    assert_eq!(ihs_wl_set_event_port(None, 0), 0);
    mock_host::dispose_view(1);
}
