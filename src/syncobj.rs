// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! The DRM device explicit sync imports clients' timelines on.
//!
//! A syncobj file is not tied to the device that made it: any DRM node that
//! does timeline syncobjs and their eventfds can import one and wait on it.
//! So the first render node that can serves, whichever GPU the client and
//! the shell render on. Render nodes only: opening a primary node could
//! contend with the shell for DRM master.

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;

use smithay::backend::drm::DrmDeviceFd;
use smithay::utils::DeviceFd;
use smithay::wayland::drm_syncobj::supports_syncobj_eventfd;

/// Render nodes to try, in order: `IHS_WL_SYNCOBJ_DEVICE` alone when set,
/// else every `/dev/dri/renderD*`.
fn candidates() -> Vec<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("IHS_WL_SYNCOBJ_DEVICE") {
        return vec![path.into()];
    }
    let Ok(dir) = std::fs::read_dir("/dev/dri") else {
        return Vec::new();
    };
    let mut nodes: Vec<_> = dir
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("renderD"))
        .map(|e| e.path())
        .collect();
    nodes.sort();
    nodes
}

/// A render node that can wait on timeline syncobjs, or None: then explicit
/// sync is not offered and clients use implicit sync.
pub fn device() -> Option<DrmDeviceFd> {
    for path in candidates() {
        if !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("renderD"))
        {
            tracing::warn!(?path, "not a render node; explicit sync not offered");
            continue;
        }
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) => {
                tracing::debug!(?path, "open: {e}");
                continue;
            }
        };
        let device = DrmDeviceFd::new(DeviceFd::from(std::os::fd::OwnedFd::from(file)));
        if supports_syncobj_eventfd(&device) {
            tracing::info!(?path, "explicit sync over");
            return Some(device);
        }
        tracing::debug!(?path, "no syncobj eventfd");
    }
    tracing::info!("no render node with syncobj eventfd; explicit sync not offered");
    None
}
