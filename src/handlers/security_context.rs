// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! wp_security_context_v1: a sandbox engine (flatpak) hands the server a
//! listening socket for one app, and every client that connects through it
//! carries that app's identity.
//!
//! The listener is served on the compositor loop until the engine closes its
//! end of `close_fd`; clients already connected through it stay. A client
//! connected through a context never sees the manager, so it cannot mint a
//! context of its own -- the protocol requires that.

use smithay::delegate_security_context;
use smithay::reexports::wayland_server::{Client, DisplayHandle};
use smithay::wayland::security_context::{
    SecurityContext, SecurityContextHandler, SecurityContextListenerSource, SecurityContextState,
};

use crate::observe::{self, Observed};
use crate::state::{ClientState, State};

/// The manager global, shown to clients on the server's own socket only.
pub fn global(dh: &DisplayHandle) -> SecurityContextState {
    SecurityContextState::new::<State, _>(dh, |client: &Client| !sandboxed(client))
}

/// Whether @p client connected through a security context. One without the
/// server's client data (none today) counts as sandboxed: hiding is the safe
/// default.
pub fn sandboxed(client: &Client) -> bool {
    client
        .get_data::<ClientState>()
        .is_none_or(|data| data.security_context.is_some())
}

impl SecurityContextHandler for State {
    fn context_created(&mut self, source: SecurityContextListenerSource, context: SecurityContext) {
        tracing::info!(
            sandbox_engine = context.sandbox_engine,
            app_id = context.app_id,
            instance_id = context.instance_id,
            creator = ?context.creator_client_id,
            "security context created"
        );
        observe::emit(Observed::SecurityContextCreated {
            sandbox_engine: context.sandbox_engine.clone(),
            app_id: context.app_id.clone(),
            instance_id: context.instance_id.clone(),
        });
        let inserted = self
            .loop_handle
            .insert_source(source, move |stream, _, state| {
                state.accept_client(stream, Some(context.clone()))
            });
        if let Err(e) = inserted {
            tracing::warn!("security context listener: {}", e.error);
        }
    }
}

delegate_security_context!(State);
