// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Explicit sync: a commit waits for its acquire point, and a buffer's
//! release point is signaled once the shell is done with it. Needs a render
//! node with timeline syncobjs; skipped without one.

mod common;

use std::time::{Duration, Instant};

use common::client::Client;
use common::drm::Timeline;
use common::harness::Harness;
use common::{mock_host, serial};
use ihs_wl_server::observe::Observed;

#[test]
fn acquire_and_release_points() {
    let _serial = serial();
    let Some(timeline) = Timeline::new() else {
        eprintln!("skipped: no render node with timeline syncobjs");
        return;
    };
    let mut h = Harness::new("ihs-wl-test-syncobj");
    if !h.client.has_syncobj() {
        eprintln!("skipped: the server found no render node with syncobj eventfd");
        return;
    }
    h.client.create_toplevel("org.example.u", "u");
    let a = h.client.new_dmabuf(32, 32);
    let b = h.client.new_dmabuf(32, 32);
    h.client.commit_buffer(a, false);
    h.bind(30, "org.example.u", 32.0, 32.0);
    h.wait_submissions(30, 1);
    h.client.use_explicit_sync(timeline.export());

    // Held until the acquire point signals.
    h.client.set_sync_points(1, 2);
    h.client.commit_buffer(b, false);
    h.client.roundtrip();
    assert_eq!(
        Harness::submissions(30).len(),
        1,
        "went out before its acquire point"
    );
    timeline.signal(1);
    let id_b = h.wait_submissions(30, 2)[1].layers[0].buffer_id;

    // Explicit sync replaces implicit: a buffer whose implicit fence never
    // signals goes out once its acquire point has.
    let (c, _rendering) = h.client.new_pending_dmabuf(32, 32);
    timeline.signal(3);
    h.client.set_sync_points(3, 4);
    h.client.commit_buffer(c, false);
    let subs = h.wait_submissions(30, 3);
    assert_ne!(subs[2].layers[0].buffer_id, id_b);

    // b's release point signals once a frame after it is on screen, and
    // not before.
    assert_eq!(timeline.signaled_point(), Some(3));
    mock_host::present(30, subs[2].seq);
    let deadline = Instant::now() + Duration::from_secs(5);
    while timeline.signaled_point() < Some(2) {
        assert!(Instant::now() < deadline, "release point 2 never signaled");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        timeline.signaled_point() < Some(4),
        "c released while shown"
    );
    mock_host::dispose_view(30);
}

/// A client that leaves with a commit waiting on a point nobody will signal
/// does not leave the wait behind.
#[test]
fn a_disconnected_clients_waits_are_dropped() {
    let _serial = serial();
    let Some(timeline) = Timeline::new() else {
        eprintln!("skipped: no render node with timeline syncobjs");
        return;
    };
    let mut h = Harness::new("ihs-wl-test-syncobj-gone");
    if !h.client.has_syncobj() {
        eprintln!("skipped: the server found no render node with syncobj eventfd");
        return;
    }
    h.client.create_toplevel("org.example.v", "v");
    let a = h.client.new_dmabuf(16, 16);
    h.client.use_explicit_sync(timeline.export());
    h.client.set_sync_points(1, 2);
    h.client.commit_buffer(a, false);
    h.client.roundtrip();

    h.client = Client::connect("ihs-wl-test-syncobj-gone");
    h.rec.wait_for("WaitsDropped", |e| {
        *e == Observed::WaitsDropped { count: 1 }
    });
}
