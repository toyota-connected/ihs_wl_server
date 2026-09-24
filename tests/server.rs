// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! A client maps a toplevel under the mock host, and the server
//! starts and stops cleanly, repeatedly.

mod common;

use std::ffi::CStr;

use common::{client::Client, mock_host, serial, Recorder};
use ihs_wl_server::observe::Observed;
use ihs_wl_server::{IhsWlConfig, IhsWlResult};

const OK: i32 = IhsWlResult::Ok as i32;
const ERR_INVALID: i32 = IhsWlResult::ErrInvalid as i32;
const ERR_ALREADY_RUNNING: i32 = IhsWlResult::ErrAlreadyRunning as i32;

fn start_named(name: &str) {
    ihs_wl_server::start(ihs_wl_server::Config {
        socket_name: Some(name.into()),
    })
    .unwrap();
}

#[test]
fn maps_a_toplevel_under_the_mock_host() {
    let _serial = serial();
    mock_host::install();
    let rec = Recorder::install();
    start_named("ihs-wl-test-map");
    assert!(mock_host::has_factory("ihs_wl/toplevel"));

    let mut client = Client::connect("ihs-wl-test-map");
    for g in [
        "wl_compositor",
        "wl_subcompositor",
        "wl_shm",
        "wl_seat",
        "xdg_wm_base",
    ] {
        assert!(
            client.globals().iter().any(|x| x == g),
            "missing global {g}"
        );
    }
    client.map_toplevel("org.example.simple-shm", "simple-shm", 250, 250);
    rec.wait_for("ToplevelMapped", |e| {
        *e == Observed::ToplevelMapped {
            app_id: "org.example.simple-shm".into(),
            title: "simple-shm".into(),
        }
    });

    // The registry's side of a view: create negotiates a grant, dispose ends it.
    assert_eq!(
        mock_host::create_view("ihs_wl/toplevel", 7, 800.0, 480.0),
        0
    );
    assert_eq!(mock_host::grants(), 1);
    rec.wait_for("ViewCreated", |e| {
        *e == Observed::ViewCreated { view_id: 7 }
    });
    mock_host::resize_view(7, 1600.0, 960.0);
    mock_host::dispose_view(7);
    rec.wait_for("ViewDisposed", |e| {
        *e == Observed::ViewDisposed { view_id: 7 }
    });

    drop(client);
    rec.wait_for("ToplevelUnmapped", |e| {
        matches!(e, Observed::ToplevelUnmapped { app_id } if app_id == "org.example.simple-shm")
    });

    ihs_wl_server::stop().unwrap();
    assert!(!mock_host::has_factory("ihs_wl/toplevel"));
    mock_host::uninstall();
    common::clear_observer();
}

#[test]
fn c_abi_start_stop_x100() {
    let _serial = serial();
    let name = c"ihs-wl-test-cycle";
    let cfg = IhsWlConfig {
        struct_size: std::mem::size_of::<IhsWlConfig>(),
        socket_name: name.as_ptr(),
    };
    for i in 0..100 {
        assert_eq!(
            unsafe { ihs_wl_server::ihs_wl_start(&cfg) },
            OK,
            "start #{i}"
        );
        let bound = unsafe { CStr::from_ptr(ihs_wl_server::ihs_wl_socket_name()) };
        assert_eq!(bound, name);
        if i % 10 == 0 {
            // A live client across stop must be disconnected, not leaked.
            let mut client = Client::connect("ihs-wl-test-cycle");
            client.roundtrip();
            ihs_wl_server::ihs_wl_stop();
            assert!(client.roundtrip_fails());
        } else {
            ihs_wl_server::ihs_wl_stop();
        }
        assert!(ihs_wl_server::ihs_wl_socket_name().is_null());
    }
}

#[test]
fn c_abi_rejects_bad_config_and_double_start() {
    let _serial = serial();
    assert_eq!(
        unsafe { ihs_wl_server::ihs_wl_start(std::ptr::null()) },
        ERR_INVALID
    );
    let short = IhsWlConfig {
        struct_size: 4,
        socket_name: std::ptr::null(),
    };
    assert_eq!(unsafe { ihs_wl_server::ihs_wl_start(&short) }, ERR_INVALID);
    let msg = unsafe { CStr::from_ptr(ihs_wl_server::ihs_wl_last_error()) };
    assert!(msg.to_str().unwrap().contains("struct_size"), "{msg:?}");

    let cfg = IhsWlConfig {
        struct_size: std::mem::size_of::<IhsWlConfig>(),
        socket_name: c"ihs-wl-test-double".as_ptr(),
    };
    assert_eq!(unsafe { ihs_wl_server::ihs_wl_start(&cfg) }, OK);
    assert_eq!(
        unsafe { ihs_wl_server::ihs_wl_start(&cfg) },
        ERR_ALREADY_RUNNING
    );
    ihs_wl_server::ihs_wl_stop();
    ihs_wl_server::ihs_wl_stop(); // no-op when stopped
}
