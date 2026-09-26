// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! xdg-activation: binding a view to the toplevel launched for it.
//!
//! The launcher asks for a token (`ihs_wl_activation_token`), starts the
//! client with it in `XDG_ACTIVATION_TOKEN`, and names the same token in the
//! view's creationParams. The client activates its toplevel with the token
//! (GTK and Qt do this on their own), which reserves the toplevel for that
//! view: views binding by app_id pass it over. Tokens clients make for each
//! other reserve nothing.

use std::time::Duration;

use smithay::delegate_xdg_activation;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};

use crate::state::{State, Toplevels};

/// Marks a token the module issued, in the token's user data.
struct Issued;

/// A token no one used in this long is dropped. Only tokens clients made
/// expire: one the module issued waits for its client however long it
/// takes to start.
const CLIENT_TOKEN_LIFETIME: Duration = Duration::from_secs(60);

impl State {
    /// A new token for a client about to be launched.
    pub fn issue_activation_token(&mut self) -> String {
        let state = &mut self.activation_state;
        state.retain_tokens(|_, data| {
            data.user_data.get::<Issued>().is_some()
                || data.timestamp.elapsed() < CLIENT_TOKEN_LIFETIME
        });
        let (token, data) = state.create_external_token(None);
        data.user_data.insert_if_missing(|| Issued);
        token.to_string()
    }
}

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.activation_state
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        if token_data.user_data.get::<Issued>().is_none() {
            // Asking for focus, or a client's own token: nothing to bind.
            tracing::debug!("activation with a token the module did not issue");
            return;
        }
        let root = crate::popups::toplevel_of(&self.popups, &surface);
        let Some(id) = Toplevels::id_of(&root) else {
            return;
        };
        // One toplevel per token.
        self.activation_state.remove_token(&token);
        let token = String::from(token);
        let Some(entry) = self.toplevels.by_id.get_mut(&id) else {
            return;
        };
        tracing::info!(id, "toplevel activated with an issued token");
        entry.token = Some(token.clone());
        // Taken by a view binding by app_id before the activation came: the
        // view naming the token has the better claim.
        if let Some(view_id) = entry.view {
            let named = self
                .views
                .get(&view_id)
                .is_some_and(|v| v.handle.params.token.as_deref() == Some(&token));
            if named {
                return;
            }
            let wanted = self
                .views
                .values()
                .any(|v| v.handle.params.token.as_deref() == Some(&token));
            if !wanted {
                return;
            }
            self.unbind_view(view_id);
        }
        self.bind_waiting_views();
    }
}

delegate_xdg_activation!(State);
