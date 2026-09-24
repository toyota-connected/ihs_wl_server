// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Frame timing driven by the shell's reports of shown frames: presentation
//! feedback, fifo barriers, and commit timing.

mod common;

use std::time::{Duration, Instant};

use common::client::Feedback;
use common::harness::Harness;
use common::{mock_host, serial};

const REFRESH_NS: u32 = 16_666_667;

fn now_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// A feedback's outcome, once the client has it.
fn wait_feedback(h: &mut Harness, tag: usize) -> Feedback {
    h.client
        .dispatch_until("presentation feedback", |c| c.feedback(tag).is_some());
    h.client.feedback(tag).unwrap()
}

/// Feedback of a shown frame reports the shell's timing; that of an update
/// replaced before it was shown is discarded, and so is that of a frame
/// whose view went away.
#[test]
fn feedback_reports_the_shown_frame() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-feedback");
    h.client.create_toplevel("org.example.p", "p");
    let a = h.client.new_dmabuf(32, 32);
    let b = h.client.new_dmabuf(32, 32);
    h.client.commit_buffer(a, false);
    h.bind(20, "org.example.p", 32.0, 32.0);
    h.wait_submissions(20, 1);
    assert_eq!(
        h.client.presentation_clock(),
        Some(libc::CLOCK_MONOTONIC as u32)
    );

    let replaced = h.client.request_feedback(false);
    h.client.commit_buffer(b, false);
    let shown = h.client.request_feedback(false);
    h.client.commit_buffer(a, false);
    let seq = h.wait_submissions(20, 3)[2].seq;
    let flags = 0x1 | 0x2 | 0x8; // vsync, hw clock, zero copy
    mock_host::present_at(20, seq, 5_000_000_123, REFRESH_NS, 42, flags);
    assert_eq!(
        wait_feedback(&mut h, shown),
        Feedback::Presented {
            time_ns: 5_000_000_123,
            refresh_ns: REFRESH_NS,
            msc: 42,
            flags,
        }
    );
    assert_eq!(wait_feedback(&mut h, replaced), Feedback::Discarded);

    let unshown = h.client.request_feedback(false);
    h.client.commit_buffer(b, false);
    h.wait_submissions(20, 4);
    mock_host::dispose_view(20);
    assert_eq!(wait_feedback(&mut h, unshown), Feedback::Discarded);
}

/// A surface's update is shown with a later frame when that frame still
/// has it -- another surface of the tree moved on, not this one.
#[test]
fn feedback_of_content_still_shown_is_presented() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-carried");
    h.client.create_toplevel("org.example.q", "q");
    let base = h.client.new_dmabuf(64, 64);
    let video = h.client.new_dmabuf(16, 16);
    let video2 = h.client.new_dmabuf(16, 16);
    h.client.commit_buffer(base, false);
    h.bind(21, "org.example.q", 64.0, 64.0);
    h.wait_submissions(21, 1);
    h.client.add_subsurface(video, 4, 4);
    let n = Harness::submissions(21).len();

    // The subsurface's update goes out in one frame, and the next frame --
    // the parent's -- replaces that one before it is shown.
    let tag = h.client.request_feedback(true);
    h.client.commit_sub_buffer(video2);
    h.client.commit_buffer(base, false);
    let subs = h.wait_submissions(21, n + 2);
    mock_host::present_at(
        21,
        subs.last().unwrap().seq,
        7_000_000_000,
        REFRESH_NS,
        7,
        0,
    );
    assert!(matches!(
        wait_feedback(&mut h, tag),
        Feedback::Presented {
            time_ns: 7_000_000_000,
            ..
        }
    ));
    mock_host::dispose_view(21);
}

/// A commit that waits on the fifo barrier is held until the frame that
/// set it is shown.
#[test]
fn fifo_waits_for_the_frame_that_set_the_barrier() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-fifo");
    h.client.create_toplevel("org.example.r", "r");
    let a = h.client.new_dmabuf(32, 32);
    let b = h.client.new_dmabuf(32, 32);
    h.client.commit_buffer(a, false);
    h.bind(22, "org.example.r", 32.0, 32.0);
    h.wait_submissions(22, 1);

    h.client.fifo_set_barrier();
    h.client.commit_buffer(b, false);
    let barrier_seq = h.wait_submissions(22, 2)[1].seq;

    h.client.fifo_wait_barrier();
    h.client.fifo_set_barrier();
    h.client.commit_buffer(a, false);
    h.client.roundtrip();
    assert_eq!(
        Harness::submissions(22).len(),
        2,
        "a commit waiting on the barrier went out before its frame was shown"
    );

    mock_host::present(22, barrier_seq);
    let subs = h.wait_submissions(22, 3);
    assert_eq!(subs[2].layers[0].buffer_id, subs[0].layers[0].buffer_id);
    mock_host::dispose_view(22);
}

/// Off screen nothing is reported, so the barrier clears on the clock
/// instead: the client keeps going, at about the display's pace.
#[test]
fn fifo_barriers_clear_on_the_clock_off_screen() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-fifo-off");
    h.client.create_toplevel("org.example.s", "s");
    let a = h.client.new_dmabuf(32, 32);
    let b = h.client.new_dmabuf(32, 32);
    h.client.commit_buffer(a, false);
    h.bind(23, "org.example.s", 32.0, 32.0);
    h.wait_submissions(23, 1);
    mock_host::set_suspended(23, true);

    let start = Instant::now();
    for i in 0..5 {
        h.client.fifo_wait_barrier();
        h.client.fifo_set_barrier();
        h.client
            .commit_buffer(if i % 2 == 0 { b } else { a }, false);
    }
    h.wait_submissions(23, 6);
    let took = start.elapsed();
    assert!(
        took < Duration::from_secs(1),
        "barriers held for {took:?} off screen"
    );
    mock_host::dispose_view(23);
}

/// A commit with a target time is applied a refresh ahead of the first
/// vblank at or after it -- not sooner; one already due goes at once.
#[test]
fn a_timed_commit_waits_for_its_target() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-timed");
    h.client.create_toplevel("org.example.t", "t");
    let a = h.client.new_dmabuf(32, 32);
    let b = h.client.new_dmabuf(32, 32);
    h.client.commit_buffer(a, false);
    h.bind(24, "org.example.t", 32.0, 32.0);
    let seq = h.wait_submissions(24, 1)[0].seq;
    // Lock the clock's phase to now.
    mock_host::present_at(24, seq, now_ns(), REFRESH_NS, 1, 0);
    h.client.roundtrip();

    let target = now_ns() + 150_000_000;
    h.client.set_target(target);
    h.client.commit_buffer(b, false);
    assert_eq!(Harness::submissions(24).len(), 1, "applied before its time");
    h.wait_submissions(24, 2);
    let applied = now_ns();
    let earliest = target - REFRESH_NS as u64 - 1_000_000;
    assert!(
        applied >= earliest,
        "applied {} ms before its target",
        (target - applied) / 1_000_000
    );
    assert!(
        applied < target + 500_000_000,
        "applied {} ms after its target",
        (applied - target) / 1_000_000
    );

    h.client.set_target(now_ns() - 1_000_000);
    let start = Instant::now();
    h.client.commit_buffer(a, false);
    h.wait_submissions(24, 3);
    assert!(start.elapsed() < Duration::from_millis(500));
    mock_host::dispose_view(24);
}
