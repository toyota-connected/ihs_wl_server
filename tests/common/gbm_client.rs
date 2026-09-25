// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Client side of gbm_buffer_backend.

#![allow(non_upper_case_globals, non_camel_case_types, clippy::all)]

use wayland_client;
use wayland_client::protocol::*;

pub mod __interfaces {
    use wayland_client::protocol::__interfaces::*;
    wayland_scanner::generate_interfaces!("protocols/gbm-buffer-backend.xml");
}
use self::__interfaces::*;

wayland_scanner::generate_client_code!("protocols/gbm-buffer-backend.xml");
