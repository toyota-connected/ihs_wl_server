// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! The view's `creationParams`, as the widget encodes them with Flutter's
//! StandardMessageCodec: a map of string keys to strings, numbers or null.
//!
//! Only the value types that map uses are decoded. Any other (a list, typed
//! data) ends the decode there, keeping the entries before it -- so a newer
//! widget adding such a key after the ones this module reads never breaks
//! it.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Double(f64),
    String(String),
}

/// The view parameters the server acts on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewParams {
    /// The xdg-activation token the launched client was given.
    pub token: Option<String>,
    /// Bind the oldest unbound toplevel with this app_id.
    pub app_id: Option<String>,
    /// Device pixel ratio of the view.
    pub dpr: Option<f64>,
}

impl ViewParams {
    pub fn decode(bytes: &[u8]) -> Self {
        let map = decode_map(bytes).unwrap_or_default();
        let string = |key: &str| match map.get(key) {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        };
        ViewParams {
            token: string("token"),
            app_id: string("app_id"),
            dpr: match map.get("dpr") {
                Some(Value::Double(d)) if d.is_finite() && *d > 0.0 => Some(*d),
                Some(Value::Int(i)) if *i > 0 => Some(*i as f64),
                _ => None,
            },
        }
    }
}

// StandardMessageCodec type tags.
const T_NULL: u8 = 0;
const T_TRUE: u8 = 1;
const T_FALSE: u8 = 2;
const T_INT32: u8 = 3;
const T_INT64: u8 = 4;
const T_FLOAT64: u8 = 6;
const T_STRING: u8 = 7;
const T_MAP: u8 = 13;

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    /// The codec's variable-length size.
    fn size(&mut self) -> Option<usize> {
        match self.u8()? {
            b @ 0..=253 => Some(b as usize),
            254 => Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?) as usize),
            _ => Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?) as usize),
        }
    }

    /// Floats are 8-byte aligned from the start of the message.
    fn align(&mut self, to: usize) -> Option<()> {
        let pad = (to - self.pos % to) % to;
        self.take(pad).map(|_| ())
    }

    fn value(&mut self) -> Option<Value> {
        match self.u8()? {
            T_NULL => Some(Value::Null),
            T_TRUE => Some(Value::Bool(true)),
            T_FALSE => Some(Value::Bool(false)),
            T_INT32 => Some(Value::Int(
                i32::from_le_bytes(self.take(4)?.try_into().ok()?) as i64,
            )),
            T_INT64 => Some(Value::Int(i64::from_le_bytes(
                self.take(8)?.try_into().ok()?,
            ))),
            T_FLOAT64 => {
                self.align(8)?;
                Some(Value::Double(f64::from_le_bytes(
                    self.take(8)?.try_into().ok()?,
                )))
            }
            T_STRING => {
                let n = self.size()?;
                let s = std::str::from_utf8(self.take(n)?).ok()?;
                Some(Value::String(s.to_owned()))
            }
            // Anything else would need its own decoder to be skipped; stop.
            _ => None,
        }
    }
}

/// Decode a top-level map with string keys. Entries are kept up to the first
/// one that cannot be decoded.
fn decode_map(bytes: &[u8]) -> Option<HashMap<String, Value>> {
    let mut r = Reader { bytes, pos: 0 };
    if r.u8()? != T_MAP {
        return None;
    }
    let n = r.size()?;
    let mut out = HashMap::new();
    for _ in 0..n {
        let Some(Value::String(key)) = r.value() else {
            break;
        };
        let Some(value) = r.value() else {
            break;
        };
        out.insert(key, value);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // What StandardMessageCodec().encodeMessage({'token': null,
    // 'app_id': 'org.example.app', 'dpr': 1.5}) produces.
    fn encoded() -> Vec<u8> {
        let mut b = vec![T_MAP, 3];
        let string = |b: &mut Vec<u8>, s: &str| {
            b.push(T_STRING);
            b.push(s.len() as u8);
            b.extend_from_slice(s.as_bytes());
        };
        string(&mut b, "token");
        b.push(T_NULL);
        string(&mut b, "app_id");
        string(&mut b, "org.example.app");
        string(&mut b, "dpr");
        b.push(T_FLOAT64);
        while b.len() % 8 != 0 {
            b.push(0);
        }
        b.extend_from_slice(&1.5f64.to_le_bytes());
        b
    }

    #[test]
    fn decodes_the_widget_params() {
        let p = ViewParams::decode(&encoded());
        assert_eq!(p.token, None);
        assert_eq!(p.app_id.as_deref(), Some("org.example.app"));
        assert_eq!(p.dpr, Some(1.5));
    }

    #[test]
    fn tolerates_garbage_and_truncation() {
        assert_eq!(ViewParams::decode(&[]), ViewParams::default());
        assert_eq!(ViewParams::decode(&[0xff, 0xff]), ViewParams::default());
        let full = encoded();
        for n in 0..full.len() {
            // Never panics, whatever prefix it gets.
            let _ = ViewParams::decode(&full[..n]);
        }
    }

    #[test]
    fn long_sizes() {
        let s = "x".repeat(300);
        let mut b = vec![T_MAP, 1, T_STRING, 6];
        b.extend_from_slice(b"app_id");
        b.push(T_STRING);
        b.push(254);
        b.extend_from_slice(&(300u16).to_le_bytes());
        b.extend_from_slice(s.as_bytes());
        assert_eq!(ViewParams::decode(&b).app_id.as_deref(), Some(s.as_str()));
    }
}
