//! What the shell can import, which is what the dma-buf global offers
//! clients: a format it cannot import would only fail later, in the shell.

use crate::ffi::ihs::sys;

/// A DRM fourcc and modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatModifier {
    pub fourcc: u32,
    pub modifier: u64,
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    (code[0] as u32) | (code[1] as u32) << 8 | (code[2] as u32) << 16 | (code[3] as u32) << 24
}

/// LINEAR 8888, which every shell importer takes: the offer when the shell
/// does not say (no host, or one that predates the format list).
fn fallback() -> Vec<FormatModifier> {
    [b"AR24", b"XR24", b"AB24", b"XB24"]
        .iter()
        .map(|c| FormatModifier {
            fourcc: fourcc(c),
            modifier: 0,
        })
        .collect()
}

/// The shell's importable formats, in its order of preference. Platform
/// thread.
pub fn import_formats() -> Vec<FormatModifier> {
    let mut caps = sys::IhsPvCapabilities {
        struct_size: std::mem::size_of::<sys::IhsPvCapabilities>(),
        ..Default::default()
    };
    let rc = unsafe { sys::ihs_pv_query_capabilities(&mut caps) };
    if rc != sys::IHS_PV_OK || caps.formats.is_null() || caps.format_count == 0 {
        return fallback();
    }
    let offered = unsafe { std::slice::from_raw_parts(caps.formats, caps.format_count) };
    let mut out: Vec<FormatModifier> = Vec::with_capacity(offered.len());
    for f in offered {
        let fm = FormatModifier {
            fourcc: f.fourcc,
            modifier: f.modifier,
        };
        if !out.contains(&fm) {
            out.push(fm);
        }
    }
    out
}
