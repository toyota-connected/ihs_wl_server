// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Frame callbacks: sent when the shell reports the frame shown, and still
//! sent when it never does -- a report lost for a view on screen, or a frame
//! the shell refused -- so a client waiting on one never stalls. A view off
//! screen holds them until it is shown again, and then the same holds.

mod common;

use std::time::{Duration, Instant};

use common::harness::Harness;
use common::{mock_host, serial};

const APP: &str = "org.example.frames";

/// A 32x32 toplevel shown by view @p view, its first frame reported, and the
/// two buffers it alternates.
fn shown(socket: &str, view: i32) -> (Harness, usize, usize) {
    let mut h = Harness::new(socket);
    h.client.create_toplevel(APP, "frames");
    let a = h.client.new_dmabuf(32, 32);
    let b = h.client.new_dmabuf(32, 32);
    h.client.commit_buffer(a, false);
    h.bind(view, APP, 32.0, 32.0);
    let seq = h.wait_submissions(view, 1)[0].seq;
    mock_host::present(view, seq);
    h.client.roundtrip();
    (h, a, b)
}

/// How long until the client has had @p n frame callbacks.
fn until_done(h: &mut Harness, n: u32) -> Duration {
    let start = Instant::now();
    h.client
        .dispatch_until("frame callback", |c| c.frames_done() >= n);
    start.elapsed()
}

/// Nothing more arrives in @p d.
fn quiet_for(h: &mut Harness, d: Duration) -> u32 {
    let end = Instant::now() + d;
    while Instant::now() < end {
        h.client.roundtrip();
        std::thread::sleep(Duration::from_millis(10));
    }
    h.client.frames_done()
}

#[test]
fn a_reported_frame_answers_at_once() {
    let _serial = serial();
    let (mut h, _, b) = shown("ihs-wl-test-frames-reported", 31);
    h.client.commit_buffer(b, true);
    let seq = h.wait_submissions(31, 2)[1].seq;
    mock_host::present(31, seq);
    assert!(until_done(&mut h, 1) < Duration::from_millis(200));
    mock_host::dispose_view(31);
}

#[test]
fn a_lost_report_still_answers() {
    let _serial = serial();
    let (mut h, a, b) = shown("ihs-wl-test-frames-lost", 32);
    h.client.commit_buffer(b, true);
    h.wait_submissions(32, 2);
    // Never reported: answered once it has gone stale.
    let took = until_done(&mut h, 1);
    assert!(
        took >= Duration::from_millis(150) && took < Duration::from_secs(2),
        "answered after {took:?}"
    );
    assert_eq!(quiet_for(&mut h, Duration::from_millis(400)), 1);
    // And the next one the same way: the client keeps going.
    h.client.commit_buffer(a, true);
    until_done(&mut h, 2);
    mock_host::dispose_view(32);
}

#[test]
fn a_refused_frame_answers_at_the_next_refresh() {
    let _serial = serial();
    let (mut h, _, b) = shown("ihs-wl-test-frames-refused", 33);
    mock_host::set_refuse(true);
    h.client.commit_buffer(b, true);
    assert!(until_done(&mut h, 1) < Duration::from_millis(150));
    mock_host::set_refuse(false);
    mock_host::dispose_view(33);
}

#[test]
fn off_screen_frame_callbacks_wait_for_the_view() {
    let _serial = serial();
    let (mut h, _, b) = shown("ihs-wl-test-frames-off", 34);
    mock_host::set_suspended(34, true);
    h.client.commit_buffer(b, true);
    let seq = h.wait_submissions(34, 2)[1].seq;
    assert_eq!(quiet_for(&mut h, Duration::from_millis(500)), 0);
    // Shown again: its frame is reported, and answered.
    mock_host::set_suspended(34, false);
    mock_host::present(34, seq);
    until_done(&mut h, 1);
    mock_host::dispose_view(34);
}

#[test]
fn back_on_screen_an_unreported_frame_still_answers() {
    let _serial = serial();
    let (mut h, _, b) = shown("ihs-wl-test-frames-back", 35);
    mock_host::set_suspended(35, true);
    h.client.commit_buffer(b, true);
    h.wait_submissions(35, 2);
    assert_eq!(quiet_for(&mut h, Duration::from_millis(400)), 0);
    // Shown again, but its frame is never reported.
    mock_host::set_suspended(35, false);
    let took = until_done(&mut h, 1);
    assert!(
        took >= Duration::from_millis(150) && took < Duration::from_secs(2),
        "answered after {took:?}"
    );
    mock_host::dispose_view(35);
}
