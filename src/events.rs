// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! View events for Dart, posted to a native port.
//!
//! Dart hands over `NativeApi.postCObject` and a `ReceivePort`'s native
//! port. A post copies the message, and posting to a port whose isolate is
//! gone only fails -- unlike a `NativeCallable`, which is undefined behavior
//! to call once closed. So a hot restart, which replaces the isolate while
//! the server runs on, needs nothing more than the new isolate registering
//! its own port.
//!
//! Each message is a list `[event, view_id]`, `event` an `IhsWlViewEvent`.

use std::ffi::c_void;
use std::sync::Mutex;

/// `Dart_PostCObject`, as `NativeApi.postCObject` gives it; the message is
/// a `Dart_CObject`.
pub type PostCObject = unsafe extern "C" fn(port: i64, message: *mut c_void) -> bool;

/// The port events go to, and how to post there.
static SINK: Mutex<Option<(PostCObject, i64)>> = Mutex::new(None);

// Dart_CObject_Type (dart_native_api.h).
const K_INT64: i32 = 3;
const K_ARRAY: i32 = 6;

/// `Dart_CObject`: a type, then a union whose largest member is five
/// pointer-sized words (`as_external_typed_data`).
#[repr(C)]
pub struct CObject {
    ty: i32,
    value: CObjectValue,
}

#[repr(C)]
union CObjectValue {
    as_int64: i64,
    as_array: CArray,
    _size: [usize; 5],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CArray {
    length: isize,
    values: *mut *mut CObject,
}

fn int64(v: i64) -> CObject {
    CObject {
        ty: K_INT64,
        value: CObjectValue { as_int64: v },
    }
}

/// Post events from now on to @p port with @p post; None stops.
pub fn set_sink(sink: Option<(PostCObject, i64)>) {
    *SINK.lock().unwrap_or_else(|e| e.into_inner()) = sink;
}

/// Tell Dart @p event happened to view @p view_id, if it listens.
pub fn post(event: crate::IhsWlViewEvent, view_id: i32) {
    let Some((post, port)) = *SINK.lock().unwrap_or_else(|e| e.into_inner()) else {
        return;
    };
    let mut items = [int64(event as i64), int64(view_id as i64)];
    let mut values: [*mut CObject; 2] = [&mut items[0], &mut items[1]];
    let mut message = CObject {
        ty: K_ARRAY,
        value: CObjectValue {
            as_array: CArray {
                length: values.len() as isize,
                values: values.as_mut_ptr(),
            },
        },
    };
    // SAFETY: Dart's own poster; the message and everything it points to
    // live until it returns, and it copies them.
    if !unsafe { post(port, (&mut message as *mut CObject).cast()) } {
        // The isolate is gone (a hot restart before the new one registered):
        // nothing is listening there any more.
        tracing::debug!(port, "event port closed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_dart_cobject_layout() {
        let word = std::mem::size_of::<usize>();
        // type, padded to the union's alignment, then five words.
        assert_eq!(
            std::mem::size_of::<CObject>(),
            8 + (5 * word).next_multiple_of(8)
        );
        assert_eq!(std::mem::offset_of!(CObject, value), 8);
    }
}
