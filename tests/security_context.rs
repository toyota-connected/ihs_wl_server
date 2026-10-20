// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! wp_security_context_v1: a sandbox listener serves clients that cannot see
//! the manager, and stops when its close fd hangs up.

mod common;

use std::io::ErrorKind;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::client::Client;
use common::harness::Harness;
use common::{mock_host, serial};
use ihs_wl_server::observe::Observed;

const SOCKET: &str = "ihs-wl-test-security-context";
const SANDBOX: &str = "ihs-wl-test-security-context-app";
const MANAGER: &str = "wp_security_context_manager_v1";
const APP: &str = "org.example.Sandboxed";

fn path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR")).join(name)
}

#[test]
fn sandbox_listener() {
    let _serial = serial();
    let mut h = Harness::new(SOCKET);
    assert!(
        h.client.globals().iter().any(|g| g == MANAGER),
        "the server's own socket shows {MANAGER}"
    );

    let close = h
        .client
        .create_security_context(SANDBOX, "org.flatpak", APP, "1");
    h.rec.wait_for(
        "SecurityContextCreated",
        |e| matches!(e, Observed::SecurityContextCreated { app_id: Some(a), .. } if a == APP),
    );

    // Connected through the listener: everything but the manager.
    let mut sandboxed = Client::connect(SANDBOX);
    assert!(
        !sandboxed.globals().iter().any(|g| g == MANAGER),
        "a sandboxed client must not see {MANAGER}"
    );
    assert!(sandboxed.globals().iter().any(|g| g == "xdg_wm_base"));

    // And it is an ordinary client otherwise: its toplevel maps and binds.
    sandboxed.create_toplevel(APP, "sandboxed");
    let buf = sandboxed.new_dmabuf(64, 64);
    sandboxed.commit_buffer(buf, false);
    h.bind(1, APP, 64.0, 64.0);

    // The engine closes its end: the listener stops taking connections, and
    // the client already connected through it stays.
    drop(close);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match UnixStream::connect(path(SANDBOX)) {
            Err(e) if e.kind() == ErrorKind::ConnectionRefused => break,
            Ok(_) | Err(_) => {
                assert!(Instant::now() < deadline, "listener still accepting");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    sandboxed.roundtrip();

    mock_host::dispose_view(1);
    drop(sandboxed);
    let _ = std::fs::remove_file(path(SANDBOX));
}
