// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Shared-memory surfaces reach their view through staging dma-bufs the
//! server copies them into. Needs a render node gbm allocates on; skipped
//! without one.

mod common;

use common::harness::Harness;
use common::{mock_host, serial};

const ARGB8888: u32 = 0x3432_5241;

fn have_render_node() -> bool {
    std::fs::read_dir("/dev/dri").is_ok_and(|d| {
        d.filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
    })
}

#[test]
fn a_shm_toplevel_is_shown_through_a_staging_buffer() {
    let _serial = serial();
    if !have_render_node() {
        eprintln!("skipped: no render node");
        return;
    }
    shown_through_staging("ihs-wl-test-shm");
}

/// The same through a dma-heap -- the system one, which every kernel with
/// dma-heaps has, standing in for a contiguous one.
#[test]
fn a_shm_toplevel_is_shown_through_a_dma_heap() {
    let _serial = serial();
    if std::fs::File::open("/dev/dma_heap/system").is_err() {
        eprintln!("skipped: no system dma-heap");
        return;
    }
    struct Unset;
    impl Drop for Unset {
        fn drop(&mut self) {
            std::env::remove_var("IHS_WL_STAGING_HEAP");
        }
    }
    // Serialized with every other test, so nothing else reads it meanwhile;
    // unset again even if the test fails.
    std::env::set_var("IHS_WL_STAGING_HEAP", "system");
    let _unset = Unset;
    shown_through_staging("ihs-wl-test-shm-heap");
}

/// A single-pixel buffer goes as a 1x1 staging buffer of its premultiplied
/// color, which the shell stretches to the viewport's size.
#[test]
fn a_single_pixel_buffer_is_shown_stretched() {
    let _serial = serial();
    if !have_render_node() {
        eprintln!("skipped: no render node");
        return;
    }
    let mut h = Harness::new("ihs-wl-test-single-pixel");
    mock_host::set_probe(Some((0, 0)));
    h.client.create_toplevel("org.example.p", "p");
    h.client.set_viewport_destination(64, 32);
    h.client.commit_single_pixel([0x20, 0x40, 0x10, 0x80]);
    h.bind(15, "org.example.p", 64.0, 32.0);
    let l = &h.wait_submissions(15, 1)[0].layers[0];
    assert_eq!((l.width, l.height, l.fourcc), (1, 1, ARGB8888));
    assert_eq!(l.dst, (0, 0, 64, 32));
    assert_eq!(l.probe, Some(0x8020_4010), "ARGB, premultiplied");
    mock_host::set_probe(None);
    mock_host::dispose_view(15);
}

fn shown_through_staging(socket: &str) {
    let mut h = Harness::new(socket);
    mock_host::set_probe(Some((5, 7)));
    h.client.create_toplevel("org.example.k", "k");
    h.client.commit_shm_buffer(64, 32, 0xff20_4080);
    h.bind(14, "org.example.k", 64.0, 32.0);
    let first = h.wait_submissions(14, 1)[0].clone();
    let l = &first.layers[0];
    assert_eq!((l.width, l.height, l.fourcc), (64, 32, ARGB8888));
    assert_eq!(
        l.modifier, 0,
        "a staging buffer goes as DRM_FORMAT_MOD_LINEAR"
    );
    assert_eq!(l.dst, (0, 0, 64, 32));
    assert_eq!(l.probe, Some(0xff20_4080), "the pixels were not copied");

    // A new frame goes through the ring: its pixels arrive, in a slot of
    // its own while the first may still be on screen.
    h.client.commit_shm_buffer(64, 32, 0xff00_ff00);
    let subs = h.wait_submissions(14, 2);
    let l2 = &subs.last().unwrap().layers[0];
    assert_eq!(l2.probe, Some(0xff00_ff00));
    assert_ne!(
        l2.buffer_id, l.buffer_id,
        "a slot the shell holds was reused"
    );
    mock_host::set_probe(None);
    mock_host::dispose_view(14);
}

/// A new size drops the ring: its slots' imports are retired in the view.
#[test]
fn a_resized_shm_buffer_retires_the_old_slots() {
    let _serial = serial();
    if !have_render_node() {
        eprintln!("skipped: no render node");
        return;
    }
    let mut h = Harness::new("ihs-wl-test-shm-resize");
    h.client.create_toplevel("org.example.l", "l");
    h.client.commit_shm_buffer(16, 16, 0xffff_ffff);
    h.bind(15, "org.example.l", 32.0, 32.0);
    let old = h.wait_submissions(15, 1)[0].layers[0].buffer_id;
    h.client.commit_shm_buffer(32, 32, 0xffff_ffff);
    h.wait_submissions(15, 2);
    h.client.roundtrip();
    assert!(
        mock_host::retired().contains(&(15, old)),
        "the old slot's import was kept"
    );
    mock_host::dispose_view(15);
}

/// A slot filled a few frames back gets every change since, not only the
/// latest commit's damage.
#[test]
fn a_reused_slot_catches_up_on_the_damage_it_missed() {
    let _serial = serial();
    if !have_render_node() {
        eprintln!("skipped: no render node");
        return;
    }
    let mut h = Harness::new("ihs-wl-test-shm-age");
    mock_host::set_probe(Some((5, 7)));
    h.client.create_toplevel("org.example.m", "m");
    h.client.commit_shm_buffer(16, 16, 0xffff_0000);
    h.bind(16, "org.example.m", 16.0, 16.0);
    let a = h.wait_submissions(16, 1)[0].clone();
    h.client.commit_shm_buffer(16, 16, 0xff00_ff00);
    let b = h.wait_submissions(16, 2)[1].clone();
    // Shown: the first slot is free again.
    mock_host::present(16, b.seq);
    h.client.roundtrip();

    // All blue, but damage says only one pixel away from the probe changed.
    // The first slot missed the green frame's full damage too; without it,
    // the probe would still read red.
    h.client
        .commit_shm_damaged(16, 16, 0xff00_00ff, (0, 0, 1, 1));
    let c = h.wait_submissions(16, 3)[2].clone();
    assert_eq!(
        c.layers[0].buffer_id, a.layers[0].buffer_id,
        "the free slot was not reused"
    );
    assert_eq!(c.layers[0].probe, Some(0xff00_00ff));
    mock_host::set_probe(None);
    mock_host::dispose_view(16);
}

/// A surface that did not change is not copied again when its tree is
/// resubmitted: the same slot goes out.
#[test]
fn an_unchanged_shm_surface_reuses_its_slot() {
    let _serial = serial();
    if !have_render_node() {
        eprintln!("skipped: no render node");
        return;
    }
    let mut h = Harness::new("ihs-wl-test-shm-same");
    h.client.create_toplevel("org.example.n", "n");
    h.client.commit_shm_buffer(16, 16, 0xff11_2233);
    h.bind(17, "org.example.n", 16.0, 16.0);
    let first = h.wait_submissions(17, 1)[0].clone();
    // Resubmit without new content: resizing reconfigures, and a commit
    // with no new buffer resubmits the tree.
    h.client.commit_no_attach();
    let again = h.wait_submissions(17, 2)[1].clone();
    assert_eq!(again.layers[0].buffer_id, first.layers[0].buffer_id);
    mock_host::dispose_view(17);
}
