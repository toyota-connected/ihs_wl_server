// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Per-surface dma-buf feedback: the shell's scanout hint for a layer puts a
//! scanout tranche for its KMS device ahead of the main tranche in that
//! surface's feedback. It stays when the hint is withdrawn (the format fits
//! now) and goes when the surface leaves its view.

mod common;

use common::client::Tranche;
use common::harness::Harness;
use common::{mock_host, serial};

const APP: &str = "org.example.scanout";
const AR24: u32 = u32::from_le_bytes(*b"AR24");
const XR24: u32 = u32::from_le_bytes(*b"XR24");
const AB24: u32 = u32::from_le_bytes(*b"AB24");
const XB24: u32 = u32::from_le_bytes(*b"XB24");
const NV12: u32 = u32::from_le_bytes(*b"NV12");
/// What the mock shell imports: LINEAR 8888.
const IMPORTED: [(u32, u64); 4] = [(AR24, 0), (XR24, 0), (AB24, 0), (XB24, 0)];

/// A 64x64 toplevel shown by view 1, and its layer id.
fn shown(socket: &str, render: u64) -> (Harness, u32) {
    let mut h = Harness::with_render_device(socket, render);
    h.client.create_toplevel(APP, "scanout");
    let buf = h.client.new_dmabuf(64, 64);
    h.client.commit_buffer(buf, false);
    h.bind(1, APP, 64.0, 64.0);
    let layer_id = h.wait_submissions(1, 1)[0].layers[0].layer_id;
    (h, layer_id)
}

fn main_tranche(render: u64) -> Tranche {
    Tranche {
        device: render,
        scanout: false,
        formats: IMPORTED.to_vec(),
    }
}

#[test]
fn a_hint_adds_a_scanout_tranche_while_the_view_shows_it() {
    let _serial = serial();
    let render = libc::makedev(226, 128) as u64;
    let card = libc::makedev(226, 0) as u64;
    let (mut h, layer_id) = shown("ihs-wl-test-scanout", render);

    h.client.request_surface_feedback();
    assert_eq!(
        h.client.wait_surface_feedback(1),
        vec![main_tranche(render)]
    );

    // Only what the shell also imports goes in the tranche.
    mock_host::scanout_hint(1, layer_id, card, &[(XR24, 0), (NV12, 0), (AR24, 0)]);
    assert_eq!(
        h.client.wait_surface_feedback(2),
        vec![
            Tranche {
                device: card,
                scanout: true,
                formats: vec![(XR24, 0), (AR24, 0)],
            },
            main_tranche(render),
        ]
    );

    // The same hint again sends nothing.
    mock_host::scanout_hint(1, layer_id, card, &[(XR24, 0), (NV12, 0), (AR24, 0)]);
    assert_eq!(h.client.surface_feedback_count(), 2);

    // Withdrawn: the format fits, thanks to the tranche, which stays.
    mock_host::scanout_hint(1, layer_id, card, &[]);
    // A layer that is not in the view: ignored.
    mock_host::scanout_hint(1, layer_id + 1000, card, &[(AR24, 0)]);
    assert_eq!(h.client.surface_feedback_count(), 2);

    // Nothing the shell imports: no tranche.
    mock_host::scanout_hint(1, layer_id, card, &[(NV12, 0)]);
    assert_eq!(
        h.client.wait_surface_feedback(3),
        vec![main_tranche(render)]
    );

    // Off its view, the surface goes back to the default feedback.
    mock_host::scanout_hint(1, layer_id, card, &[(AR24, 0)]);
    assert_eq!(h.client.wait_surface_feedback(4).len(), 2);
    mock_host::dispose_view(1);
    assert_eq!(
        h.client.wait_surface_feedback(5),
        vec![main_tranche(render)]
    );
}

#[test]
fn feedback_asked_for_after_the_hint_has_the_tranche() {
    let _serial = serial();
    let render = libc::makedev(226, 128) as u64;
    let card = libc::makedev(226, 0) as u64;
    let (mut h, layer_id) = shown("ihs-wl-test-scanout-late", render);

    mock_host::scanout_hint(1, layer_id, card, &[(AR24, 0)]);
    h.client.roundtrip();
    h.client.request_surface_feedback();
    assert_eq!(
        h.client.wait_surface_feedback(1),
        vec![
            Tranche {
                device: card,
                scanout: true,
                formats: vec![(AR24, 0)],
            },
            main_tranche(render),
        ]
    );
    mock_host::dispose_view(1);
}
