// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

use std::fmt;

use crate as abi;

/// An error crossing the C ABI: a result code plus the message
/// `ihs_wl_last_error` reports.
#[derive(Debug)]
pub struct Error {
    pub code: i32,
    pub message: String,
}

impl Error {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Error {
            code,
            message: message.into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(abi::IhsWlResult::ErrInvalid as i32, message)
    }
    pub fn io(message: impl Into<String>) -> Self {
        Self::new(abi::IhsWlResult::ErrIo as i32, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(abi::IhsWlResult::ErrInternal as i32, message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
