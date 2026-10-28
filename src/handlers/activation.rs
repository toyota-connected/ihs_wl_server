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
//!
//! A window activated with a client's own token asks to be shown:
//! `ActivationRequested` when the token was made for user input in the
//! focused view and is used, while that view still has focus, on another
//! client's window; else `AttentionRequested`.

use std::time::{Duration, Instant};

use smithay::delegate_xdg_activation;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::Serial;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};

use crate::state::{State, Toplevels};

/// Marks a token the module issued, in the token's user data.
struct Issued;

/// Marks a token made for user input by the client whose view had focus.
struct Granted;

/// A token no one used in this long is dropped. Only tokens clients made
/// expire: one the module issued waits for its client however long it
/// takes to start.
const CLIENT_TOKEN_LIFETIME: Duration = Duration::from_secs(60);

/// A request to be shown, as its token stood when it was used.
pub struct Asked {
    maker: Option<ClientId>,
    /// Made for user input in the focused view.
    granted: bool,
    at: Instant,
}

/// AttentionRequested is posted for a window at most this often.
const ATTENTION_INTERVAL: Duration = Duration::from_secs(1);

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

    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        let focused = self.focused_client();
        let granted = focused.is_some()
            && focused == data.client_id
            && data
                .serial
                .as_ref()
                .is_some_and(|(s, _)| self.is_input_serial(*s));
        if granted {
            data.user_data.insert_if_missing(|| Granted);
        }
        tracing::debug!(granted, "client made an activation token");
        true
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        if token_data.user_data.get::<Issued>().is_none() {
            // A client's own token: nothing to bind.
            self.activation_state.remove_token(&token);
            if token_data.timestamp.elapsed() >= CLIENT_TOKEN_LIFETIME {
                tracing::debug!("activation with an expired token");
                return;
            }
            let asked = Asked {
                maker: token_data.client_id.clone(),
                granted: token_data.user_data.get::<Granted>().is_some(),
                at: token_data.timestamp,
            };
            self.window_asks_to_be_shown(&surface, asked);
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

impl State {
    /// The client of what the focused, activated view shows.
    pub fn focused_client(&self) -> Option<ClientId> {
        self.focused_view
            .filter(|v| self.view_activated(*v))
            .and_then(|v| self.focus_window(v))
            .and_then(|w| w.client())
            .map(|c| c.id())
    }

    /// Whether @p serial is no older than the input that gave the focused
    /// client focus and no newer than the last input.
    fn is_input_serial(&self, serial: Serial) -> bool {
        let Some(last) = self.devices.input_serial else {
            return false;
        };
        let floor = self.devices.focus.as_ref().and_then(|(_, s)| *s);
        floor.is_none_or(|f| serial.is_no_older_than(&f)) && last.is_no_older_than(&serial)
    }

    /// @p surface's window asks to be shown; held until it is a window.
    fn window_asks_to_be_shown(&mut self, surface: &WlSurface, asked: Asked) {
        let root = self.window_root(surface);
        let Some(id) = Toplevels::id_of(&root) else {
            return;
        };
        let Some(t) = self.toplevels.by_id.get_mut(&id) else {
            return;
        };
        if !t.window {
            if t.surface.parent().is_none() {
                t.asked = Some(asked);
            }
            return;
        }
        self.ask_to_be_shown(id, asked);
    }

    /// Post that the window @p id asks to be shown, unless it is active:
    /// activation if the token's maker still has focus and the window is
    /// another client's, else attention, at most once an interval.
    pub fn ask_to_be_shown(&mut self, id: i64, asked: Asked) {
        let Some(t) = self.toplevels.by_id.get(&id) else {
            return;
        };
        if t.view.is_some_and(|v| self.view_activated(v)) {
            tracing::debug!(id, "activation of the active window");
            return;
        }
        if asked.at.elapsed() >= CLIENT_TOKEN_LIFETIME {
            tracing::debug!(id, "activation with an expired token");
            return;
        }
        let owner = t.surface.wl_surface().client().map(|c| c.id());
        let granted = asked.granted
            && asked.maker.is_some()
            && self.focused_client() == asked.maker
            && owner != asked.maker;
        let t = self.toplevels.by_id.get_mut(&id).unwrap();
        let event = if granted {
            crate::IhsWlToplevelEvent::ActivationRequested
        } else {
            if t.attention_at
                .is_some_and(|at| at.elapsed() < ATTENTION_INTERVAL)
            {
                tracing::debug!(id, "attention asked again too soon");
                return;
            }
            t.attention_at = Some(Instant::now());
            crate::IhsWlToplevelEvent::AttentionRequested
        };
        tracing::info!(id, ?event, "window asks to be shown");
        crate::events::post_toplevel(event, id);
    }
}

delegate_xdg_activation!(State);
