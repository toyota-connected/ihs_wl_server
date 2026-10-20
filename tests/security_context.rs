// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! wp_security_context_v1: a sandbox listener serves clients that cannot see
//! the manager, and stops when its close fd hangs up; a view can require
//! the sandbox's app_id, and reports who its client is.

mod common;

use std::ffi::CStr;
use std::io::ErrorKind;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::client::Client;
use common::harness::Harness;
use common::{mock_host, serial};
use ihs_wl_server::observe::Observed;
use ihs_wl_server::{ihs_wl_view_client, IhsWlClientInfo, Sandbox};

const SOCKET: &str = "ihs-wl-test-security-context";
const SANDBOX: &str = "ihs-wl-test-security-context-app";
const MANAGER: &str = "wp_security_context_manager_v1";
const APP: &str = "org.example.Sandboxed";

/// creationParams for a map of string @p entries.
fn params(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut b = vec![13u8, entries.len() as u8];
    for s in entries.iter().flat_map(|(k, v)| [k, v]) {
        b.push(7);
        b.push(s.len() as u8);
        b.extend_from_slice(s.as_bytes());
    }
    b
}

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

#[test]
fn sandbox_identity() {
    const SOCKET: &str = "ihs-wl-test-sandbox-identity";
    const SANDBOX: &str = "ihs-wl-test-sandbox-identity-app";
    let _serial = serial();
    let mut h = Harness::new(SOCKET);

    // A client on the server's own socket claiming the sandboxed app's
    // app_id, mapped first: the oldest by app_id.
    h.client.create_toplevel(APP, "impostor");
    let buf = h.client.new_dmabuf(64, 64);
    h.client.commit_buffer(buf, false);
    h.rec.wait_for("impostor mapped", |e| {
        matches!(e, Observed::ToplevelMapped { title, sandbox: None, .. } if title == "impostor")
    });

    let _close = h
        .client
        .create_security_context(SANDBOX, "org.flatpak", APP, "7");
    let mut sandboxed = Client::connect(SANDBOX);
    sandboxed.create_toplevel(APP, "sandboxed");
    let buf = sandboxed.new_dmabuf(64, 64);
    sandboxed.commit_buffer(buf, false);
    h.rec.wait_for("sandboxed mapped", |e| {
        *e == Observed::ToplevelMapped {
            app_id: APP.into(),
            title: "sandboxed".into(),
            sandbox: Some((Some("org.flatpak".into()), Some(APP.into()))),
        }
    });

    // Naming the sandbox's app_id passes over the impostor.
    h.bind_with(1, &params(&[("sandbox_app_id", APP)]), 64.0, 64.0);
    let sandbox = Some(Sandbox {
        engine: Some("org.flatpak".into()),
        app_id: Some(APP.into()),
        instance_id: Some("7".into()),
    });
    let client = ihs_wl_server::view_client(1)
        .unwrap()
        .expect("view 1 bound");
    assert_eq!(client.sandbox, sandbox);
    assert_eq!(client.pid, std::process::id() as i32);

    // The app_id alone binds the impostor, which is in no sandbox.
    h.bind(2, APP, 64.0, 64.0);
    let client = ihs_wl_server::view_client(2)
        .unwrap()
        .expect("view 2 bound");
    assert_eq!(client.sandbox, None);

    // A sandbox no client is in: nothing binds.
    assert_eq!(
        mock_host::create_view_with_params(
            "ihs_wl/toplevel",
            3,
            64.0,
            64.0,
            &params(&[("sandbox_app_id", "org.example.Other")]),
        ),
        0
    );
    assert_eq!(ihs_wl_server::view_client(3).unwrap(), None);

    // The C ABI copies the same.
    let mut info: IhsWlClientInfo = unsafe { std::mem::zeroed() };
    info.struct_size = std::mem::size_of::<IhsWlClientInfo>();
    assert_eq!(unsafe { ihs_wl_view_client(1, &mut info) }, 1);
    assert!(info.sandboxed);
    let text = |s: &[std::ffi::c_char; 256]| {
        unsafe { CStr::from_ptr(s.as_ptr()) }
            .to_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(text(&info.sandbox_engine), "org.flatpak");
    assert_eq!(text(&info.sandbox_app_id), APP);
    assert_eq!(text(&info.sandbox_instance_id), "7");
    assert_eq!(unsafe { ihs_wl_view_client(2, &mut info) }, 1);
    assert!(!info.sandboxed);
    assert_eq!(text(&info.sandbox_app_id), "");
    assert_eq!(unsafe { ihs_wl_view_client(3, &mut info) }, 0);
    info.struct_size = 8;
    assert_eq!(
        unsafe { ihs_wl_view_client(1, &mut info) },
        ihs_wl_server::IhsWlResult::ErrInvalid as i32
    );

    for view in 1..=3 {
        mock_host::dispose_view(view);
    }
    drop(sandboxed);
    let _ = std::fs::remove_file(path(SANDBOX));
}

#[test]
fn sandboxed_app_id_is_its_sandbox() {
    const SOCKET: &str = "ihs-wl-test-sandbox-claim";
    const SANDBOX: &str = "ihs-wl-test-sandbox-claim-app";
    const OTHER: &str = "org.example.Other";
    let _serial = serial();
    let mut h = Harness::new(SOCKET);

    // A sandboxed client claiming another app's app_id.
    let _close = h
        .client
        .create_security_context(SANDBOX, "org.flatpak", APP, "1");
    let mut sandboxed = Client::connect(SANDBOX);
    sandboxed.create_toplevel(OTHER, "claimant");
    let buf = sandboxed.new_dmabuf(64, 64);
    sandboxed.commit_buffer(buf, false);
    h.rec.wait_for(
        "claimant mapped",
        |e| matches!(e, Observed::ToplevelMapped { title, .. } if title == "claimant"),
    );

    // A view for that app_id passes it over, and binds the app's own
    // toplevel once it maps.
    assert_eq!(
        mock_host::create_view_with_params(
            "ihs_wl/toplevel",
            1,
            64.0,
            64.0,
            &params(&[("app_id", OTHER)])
        ),
        0
    );
    assert_eq!(ihs_wl_server::view_client(1).unwrap(), None);
    h.client.create_toplevel(OTHER, "other");
    let buf = h.client.new_dmabuf(64, 64);
    h.client.commit_buffer(buf, false);
    h.rec.wait_for("ViewBound 1", |e| {
        matches!(e, Observed::ViewBound { view_id: 1, .. })
    });
    let client = ihs_wl_server::view_client(1)
        .unwrap()
        .expect("view 1 bound");
    assert_eq!(client.sandbox, None);

    // Its sandbox's app_id still binds it.
    h.bind_with(2, &params(&[("sandbox_app_id", APP)]), 64.0, 64.0);
    let client = ihs_wl_server::view_client(2)
        .unwrap()
        .expect("view 2 bound");
    assert!(client.sandbox.is_some());

    for view in 1..=2 {
        mock_host::dispose_view(view);
    }
    drop(sandboxed);
    let _ = std::fs::remove_file(path(SANDBOX));
}

#[test]
fn oversized_identity_refused() {
    const SOCKET: &str = "ihs-wl-test-sandbox-oversized";
    const SANDBOX: &str = "ihs-wl-test-sandbox-oversized-app";
    let _serial = serial();
    let mut h = Harness::new(SOCKET);
    // One byte more than IhsWlClientInfo holds.
    let long = "a".repeat(256);
    let _close = h
        .client
        .create_security_context(SANDBOX, "org.flatpak", &long, "1");
    // The listener is closed rather than served with the app_id cut short.
    let err = UnixStream::connect(path(SANDBOX)).expect_err("listener refused");
    assert_eq!(err.kind(), ErrorKind::ConnectionRefused);
    h.client.roundtrip();
    let _ = std::fs::remove_file(path(SANDBOX));
}
