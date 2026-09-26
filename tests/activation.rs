// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Binding by activation token: a view naming a token the module issued
//! binds the toplevel activated with it, whatever else maps, and views
//! binding by app_id pass that toplevel over.

mod common;

use std::ffi::{c_char, CStr};

use common::client::Client;
use common::harness::{params, params_token, Harness};
use common::{mock_host, serial};
use ihs_wl_server::observe::Observed;
use ihs_wl_server::{ihs_wl_activation_token, IhsWlResult};

const APP: &str = "org.example.activation";

fn token() -> String {
    let mut buf = [0 as c_char; 64];
    let len = unsafe { ihs_wl_activation_token(buf.as_mut_ptr(), buf.len()) };
    assert!(len > 0, "ihs_wl_activation_token: {len}");
    let token = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_str().unwrap();
    assert_eq!(token.len(), len as usize);
    token.to_owned()
}

/// A client with a mapped @p app_id toplevel.
fn mapped(socket: &str, app_id: &str) -> Client {
    let mut c = Client::connect(socket);
    c.create_toplevel(app_id, "activation");
    let buf = c.new_dmabuf(64, 64);
    c.commit_buffer(buf, false);
    c
}

fn create(view: i32, params: &[u8]) {
    assert_eq!(
        mock_host::create_view_with_params("ihs_wl/toplevel", view, 64.0, 64.0, params),
        0
    );
}

/// The toplevel view @p view bound, waiting for it.
fn bound(h: &Harness, view: i32) -> i64 {
    match h.rec.wait_for(
        "ViewBound",
        |e| matches!(e, Observed::ViewBound { view_id, .. } if *view_id == view),
    ) {
        Observed::ViewBound { toplevel_id, .. } => toplevel_id,
        _ => unreachable!(),
    }
}

fn bindings(h: &Harness, view: i32) -> Vec<i64> {
    h.rec
        .events()
        .into_iter()
        .filter_map(|e| match e {
            Observed::ViewBound {
                view_id,
                toplevel_id,
            } if view_id == view => Some(toplevel_id),
            _ => None,
        })
        .collect()
}

#[test]
fn a_token_view_binds_the_toplevel_activated_with_it() {
    let _serial = serial();
    let socket = "ihs-wl-test-activation";
    let mut h = Harness::new(socket);
    let t = token();
    // Older, same app_id, not launched with the token.
    h.client.create_toplevel(APP, "a");
    let buf = h.client.new_dmabuf(64, 64);
    h.client.commit_buffer(buf, false);
    let mut b = mapped(socket, APP);
    b.activate(&t);

    create(1, &params_token(&t, Some(APP)));
    assert_eq!(bound(&h, 1), 2);
    // An app_id view gets the other one.
    create(2, &params(APP));
    assert_eq!(bound(&h, 2), 1);
    mock_host::dispose_view(1);
    mock_host::dispose_view(2);
    drop(b);
}

#[test]
fn the_token_view_takes_its_toplevel_from_an_app_id_view() {
    let _serial = serial();
    let socket = "ihs-wl-test-activation-late";
    let h = Harness::new(socket);
    let t = token();
    create(1, &params(APP));
    create(2, &params_token(&t, None));
    // Maps before it activates: the app_id view takes it first.
    let mut b = mapped(socket, APP);
    assert_eq!(bound(&h, 1), 1);
    b.activate(&t);
    assert_eq!(bound(&h, 2), 1);
    // And it is not given back.
    b.roundtrip();
    assert_eq!(bindings(&h, 1), vec![1]);
    mock_host::dispose_view(1);
    mock_host::dispose_view(2);
}

#[test]
fn a_token_view_waits_for_its_client() {
    let _serial = serial();
    let socket = "ihs-wl-test-activation-wait";
    let mut h = Harness::new(socket);
    let t = token();
    create(1, &params_token(&t, Some(APP)));
    h.client.create_toplevel(APP, "not launched with it");
    let buf = h.client.new_dmabuf(64, 64);
    h.client.commit_buffer(buf, false);
    // A token the client made itself binds nothing.
    let own = h.client.client_token();
    h.client.activate(&own);
    h.client.roundtrip();
    assert!(bindings(&h, 1).is_empty());

    let mut b = mapped(socket, APP);
    b.activate(&t);
    assert_eq!(bound(&h, 1), 2);
    // Used up: another toplevel activating with it binds nothing more.
    h.client.activate(&t);
    assert_eq!(bindings(&h, 1), vec![2]);
    mock_host::dispose_view(1);
}

#[test]
fn a_token_needs_room() {
    let _serial = serial();
    let _h = Harness::new("ihs-wl-test-activation-room");
    let mut small = [0 as c_char; 8];
    assert_eq!(
        unsafe { ihs_wl_activation_token(small.as_mut_ptr(), small.len()) },
        IhsWlResult::ErrTooSmall as i32
    );
    assert_eq!(
        unsafe { ihs_wl_activation_token(std::ptr::null_mut(), 64) },
        IhsWlResult::ErrInvalid as i32
    );
}

#[test]
fn no_token_without_a_server() {
    let _serial = serial();
    let mut buf = [0 as c_char; 64];
    assert_eq!(
        unsafe { ihs_wl_activation_token(buf.as_mut_ptr(), buf.len()) },
        IhsWlResult::ErrNotRunning as i32
    );
}
