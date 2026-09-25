// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! A view's device pixel ratio reaches its client: the toplevel is
//! configured in logical pixels, every surface is told the scale, and a
//! buffer rendered at it is shown 1:1.

mod common;

use common::harness::{params_dpr, Harness};
use common::{mock_host, serial};

const APP: &str = "org.example.scale";

#[test]
fn a_view_tells_its_client_its_scale() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-scale");
    h.client.create_toplevel(APP, "scale");
    h.client.use_fractional_scale();
    let buf = h.client.new_dmabuf(960, 720);
    h.client.commit_buffer(buf, false);
    h.bind_with(1, &params_dpr(APP, 1.5), 640.0, 480.0);

    // Created 640x480 logical: 960x720 physical, configured back to logical.
    h.client.dispatch_until("scale and logical configure", |c| {
        c.preferred_scale() == Some(180)
            && c.buffer_scale() == Some(2)
            && c.configure_size() == Some((640, 480))
    });
    assert!(h.client.entered(false) > 0, "never entered the output");

    // Resize reports physical pixels.
    mock_host::resize_view(1, 1200.0, 900.0);
    h.client.dispatch_until("configure to 1200x900 / 1.5", |c| {
        c.configure_size() == Some((800, 600))
    });
    mock_host::dispose_view(1);
}

#[test]
fn a_buffer_at_the_view_scale_is_shown_one_to_one() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-scale-layers");
    h.client.create_toplevel(APP, "scale");
    h.bind_with(1, &params_dpr(APP, 1.5), 640.0, 480.0);

    // A fractional-aware client: a 960x720 buffer shown at 640x480 logical.
    let buf = h.client.new_dmabuf(960, 720);
    h.client.set_viewport_destination(640, 480);
    h.client.commit_buffer(buf, false);
    // A subsurface that ignores the scale is stretched to it.
    let sub = h.client.new_dmabuf(20, 20);
    h.client.add_subsurface(sub, 10, 10);
    h.client.commit_sub_buffer(sub);

    let subs = h.wait_submissions(1, 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut layers = subs.last().unwrap().layers.clone();
    while layers.len() < 2 {
        assert!(std::time::Instant::now() < deadline, "no subsurface layer");
        std::thread::sleep(std::time::Duration::from_millis(5));
        layers = Harness::submissions(1).last().unwrap().layers.clone();
    }
    assert_eq!(layers[0].src, (0, 0, 960 << 16, 720 << 16));
    assert_eq!(layers[0].dst, (0, 0, 960, 720), "the toplevel, 1:1");
    assert_eq!(layers[1].dst, (15, 15, 30, 30), "the subsurface, scaled");
    mock_host::dispose_view(1);
}
