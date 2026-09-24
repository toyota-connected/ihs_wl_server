// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! A running server with one client connected, and the views' submissions.

use super::client::Client;
use super::{mock_host, Recorder};

/// creationParams as the widget's StandardMessageCodec encodes
/// `{'app_id': app_id}`.
pub fn params(app_id: &str) -> Vec<u8> {
    let mut b = vec![13u8, 1]; // map, 1 entry
    for s in ["app_id", app_id] {
        b.push(7); // string
        b.push(s.len() as u8);
        b.extend_from_slice(s.as_bytes());
    }
    b
}

pub struct Harness {
    pub rec: Recorder,
    pub client: Client,
}

impl Harness {
    pub fn new(socket: &str) -> Self {
        mock_host::install();
        let rec = Recorder::install();
        ihs_wl_server::start(ihs_wl_server::Config {
            socket_name: Some(socket.into()),
        })
        .unwrap();
        let client = Client::connect(socket);
        Harness { rec, client }
    }

    /// Create view @p view bound by @p app_id, and wait for it to bind.
    pub fn bind(&self, view: i32, app_id: &str, width: f64, height: f64) {
        assert_eq!(
            mock_host::create_view_with_params(
                "ihs_wl/toplevel",
                view,
                width,
                height,
                &params(app_id)
            ),
            0
        );
        self.rec.wait_for("ViewBound", |e| {
            matches!(e, ihs_wl_server::observe::Observed::ViewBound { view_id, .. } if *view_id == view)
        });
    }

    /// The submissions for @p view so far.
    pub fn submissions(view: i32) -> Vec<mock_host::Submission> {
        mock_host::submissions()
            .into_iter()
            .filter(|s| s.view_id == view)
            .collect()
    }

    pub fn wait_submissions(&self, view: i32, n: usize) -> Vec<mock_host::Submission> {
        self.rec
            .wait_for("submission", |_| Self::submissions(view).len() >= n);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let s = Self::submissions(view);
            if s.len() >= n {
                return s;
            }
            assert!(std::time::Instant::now() < deadline, "no submission {n}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        ihs_wl_server::stop().unwrap();
        mock_host::uninstall();
        super::clear_observer();
    }
}
