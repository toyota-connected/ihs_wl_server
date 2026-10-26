// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! A Rust-only hook for watching the server from tests. The shell path needs
//! none of this: a view finds its toplevel by activation token or app_id, so
//! there is no event stream to Dart.

use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
pub enum Observed {
    ClientCount(u32),
    ToplevelMapped {
        app_id: String,
        title: String,
        /// The client's sandbox engine and app_id, when it connected
        /// through a security context.
        sandbox: Option<(Option<String>, Option<String>)>,
    },
    ToplevelUnmapped {
        app_id: String,
    },
    ViewCreated {
        view_id: i32,
    },
    ViewDisposed {
        view_id: i32,
    },
    ViewBound {
        view_id: i32,
        toplevel_id: i64,
    },
    Submitted {
        view_id: i32,
        seq: u64,
        layers: usize,
    },
    Retired {
        view_id: i32,
        buffer_id: u32,
    },
    /// A disconnected client's commits were still waiting for @p count
    /// buffers; the waits are dropped.
    WaitsDropped {
        count: usize,
    },
    /// A client made a sandbox listener (wp_security_context_v1).
    SecurityContextCreated {
        sandbox_engine: Option<String>,
        app_id: Option<String>,
        instance_id: Option<String>,
    },
}

pub type Observer = Arc<dyn Fn(&Observed) + Send + Sync>;

static OBSERVER: Mutex<Option<Observer>> = Mutex::new(None);

/// Install (or clear) the process-wide observer.
pub fn set_observer(observer: Option<Observer>) {
    let old = std::mem::replace(
        &mut *OBSERVER.lock().unwrap_or_else(|e| e.into_inner()),
        observer,
    );
    // Dropped outside the lock: the old observer's drop may run arbitrary code.
    drop(old);
}

pub(crate) fn emit(event: Observed) {
    tracing::debug!(?event, "observed");
    let observer = OBSERVER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(observer) = observer {
        observer(&event);
    }
}
