// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! What the shell can import, which is what the dma-buf global offers
//! clients: a format it cannot import would only fail later, in the shell.
//! And the GPU it imports on, which clients should allocate on.

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

/// What the shell reports.
#[derive(Clone)]
pub struct Caps {
    /// Importable formats, in the shell's order of preference.
    pub formats: Vec<FormatModifier>,
    /// The render node it imports on (a dev_t), when it can tell.
    pub render_device: Option<u64>,
    /// The shell's EGL display and config, when it samples image layers
    /// (IHS_PV_KIND_TEXTURE_EGL_IMAGE): EGLImages are made on that display.
    #[cfg_attr(not(feature = "egl-wl-display"), allow(dead_code))]
    pub egl_images: Option<ShellEgl>,
}

/// The shell's EGLDisplay and EGLConfig, as addresses.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(feature = "egl-wl-display"), allow(dead_code))]
pub struct ShellEgl {
    pub display: usize,
    pub config: usize,
}

/// Ask the shell. Platform thread.
pub fn query() -> Caps {
    let mut caps = sys::IhsPvCapabilities {
        struct_size: std::mem::size_of::<sys::IhsPvCapabilities>(),
        ..Default::default()
    };
    let rc = unsafe { sys::ihs_pv_query_capabilities(&mut caps) };
    let render_device =
        (rc == sys::IHS_PV_OK && caps.render_device != 0).then_some(caps.render_device);
    let egl_images = (rc == sys::IHS_PV_OK && caps.kinds & sys::IHS_PV_KIND_TEXTURE_EGL_IMAGE != 0)
        .then(shell_egl)
        .flatten();
    Caps {
        formats: import_formats(rc, &caps),
        render_device,
        egl_images,
    }
}

/// The shell's EGL display and config. Platform thread.
fn shell_egl() -> Option<ShellEgl> {
    let mut egl = sys::IhsEglContext {
        struct_size: std::mem::size_of::<sys::IhsEglContext>(),
        ..Default::default()
    };
    let rc = unsafe { sys::ihs_pv_egl_context(&mut egl) };
    (rc == sys::IHS_PV_OK && !egl.egl_display.is_null() && !egl.egl_config.is_null()).then_some(
        ShellEgl {
            display: egl.egl_display as usize,
            config: egl.egl_config as usize,
        },
    )
}

fn import_formats(rc: i32, caps: &sys::IhsPvCapabilities) -> Vec<FormatModifier> {
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

impl Caps {
    /// The shell lists @p fourcc with @p modifier as importable.
    pub fn accepts(&self, fourcc: u32, modifier: u64) -> bool {
        self.formats
            .iter()
            .any(|f| f.fourcc == fourcc && f.modifier == modifier)
    }
}
