// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Bindings to the ivi-homescreen shared ABI (`libihs_shared.so.1`), generated
//! by build.rs from the installed `ihs/*.h`.

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]
pub mod sys {
    include!(concat!(env!("OUT_DIR"), "/ihs_bindings.rs"));
}
