// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! The DRM device explicit sync imports clients' timelines on.
//!
//! A syncobj file is not tied to the device that made it: any DRM node that
//! does timeline syncobjs and their eventfds can import one and wait on it.
//! So the first render node that can serves, whichever GPU the client and
//! the shell render on.

use std::fs::File;
use std::os::fd::AsFd;

use smithay::backend::drm::DrmDeviceFd;
use smithay::utils::DeviceFd;

/// Whether @p node can make eventfds for syncobj points. Asked of the file
/// directly: smithay's own check needs a DrmDeviceFd, whose constructor
/// tries for DRM master and warns when a render node refuses, as they
/// always do.
fn supports_syncobj_eventfd(node: &File) -> bool {
    // Handle 0 is never valid: a device with the ioctl answers ENOENT, one
    // without it (or without timeline syncobjs) something else. The fd
    // stands in for the eventfd, since drm-ffi needs a valid one.
    match drm_ffi::syncobj::eventfd(node.as_fd(), 0, 0, node.as_fd(), false) {
        Ok(_) => false,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// A render node that can wait on timeline syncobjs, or None: then explicit
/// sync is not offered and clients use implicit sync.
pub fn device() -> Option<DrmDeviceFd> {
    for (path, file) in crate::nodes::render_nodes("IHS_WL_SYNCOBJ_DEVICE") {
        if !supports_syncobj_eventfd(&file) {
            tracing::debug!(?path, "no syncobj eventfd");
            continue;
        }
        tracing::info!(?path, "explicit sync over");
        return Some(DrmDeviceFd::new(DeviceFd::from(
            std::os::fd::OwnedFd::from(file),
        )));
    }
    tracing::info!("no render node with syncobj eventfd; explicit sync not offered");
    None
}
