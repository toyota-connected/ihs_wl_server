// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! linux-dmabuf feedback: v4 with the shell's render device as main_device
//! when the shell reports one, v3 when it does not.

mod common;

use common::client::Client;
use common::{mock_host, serial};

fn start(socket: &str, render_device: u64) -> Client {
    mock_host::install();
    mock_host::set_render_device(render_device);
    ihs_wl_server::start(ihs_wl_server::Config {
        socket_name: Some(socket.into()),
    })
    .unwrap();
    Client::connect(socket)
}

fn stop() {
    ihs_wl_server::stop().unwrap();
    mock_host::set_render_device(0);
    mock_host::uninstall();
}

#[test]
fn the_render_device_is_the_main_device() {
    let _serial = serial();
    // 226:128, the first render node.
    let dev = libc::makedev(226, 128) as u64;
    let mut client = start("ihs-wl-test-feedback-v4", dev);
    assert!(client.dmabuf_version() >= 4);
    assert_eq!(client.default_feedback_main_device(), Some(dev));
    drop(client);
    stop();
}

#[test]
fn no_render_device_offers_v3() {
    let _serial = serial();
    let client = start("ihs-wl-test-feedback-v3", 0);
    assert_eq!(client.dmabuf_version(), 3);
    drop(client);
    stop();
}
