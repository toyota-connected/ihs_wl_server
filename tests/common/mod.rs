// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

#![allow(dead_code)]

pub mod client;
pub mod drm;
pub mod harness;
pub mod mock_host;

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ihs_wl_server::observe::{self, Observed};

/// The server is process-global; tests in one binary take turns.
pub fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Collects everything the server reports through the observer hook.
#[derive(Clone, Default)]
pub struct Recorder(Arc<(Mutex<Vec<Observed>>, Condvar)>);

impl Recorder {
    pub fn install() -> Self {
        let rec = Recorder::default();
        let sink = rec.clone();
        observe::set_observer(Some(Arc::new(move |e: &Observed| {
            let (events, cv) = &*sink.0;
            events.lock().unwrap().push(e.clone());
            cv.notify_all();
        })));
        rec
    }

    /// Everything seen so far.
    pub fn events(&self) -> Vec<Observed> {
        self.0 .0.lock().unwrap().clone()
    }

    /// Wait until an event matching `pred` has been seen.
    pub fn wait_for(&self, what: &str, pred: impl Fn(&Observed) -> bool) -> Observed {
        let (events, cv) = &*self.0;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut guard = events.lock().unwrap();
        loop {
            if let Some(e) = guard.iter().find(|e| pred(e)) {
                return e.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "timed out waiting for {what}; saw {:?}",
                *guard
            );
            guard = cv.wait_timeout(guard, left).unwrap().0;
        }
    }
}

/// Clear the observer installed by `Recorder::install`.
pub fn clear_observer() {
    observe::set_observer(None);
}
