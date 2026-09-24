// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! A DRM timeline syncobj, as an explicit-sync client makes one, on the
//! first render node that has them.

use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, FromRawFd, OwnedFd};

pub struct Timeline {
    device: File,
    handle: u32,
}

impl Timeline {
    /// None when there is no render node with timeline syncobjs.
    pub fn new() -> Option<Timeline> {
        let mut nodes: Vec<_> = std::fs::read_dir("/dev/dri")
            .ok()?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("renderD"))
            .map(|e| e.path())
            .collect();
        nodes.sort();
        for path in nodes {
            let Ok(device) = OpenOptions::new().read(true).write(true).open(&path) else {
                continue;
            };
            let Ok(created) = drm_ffi::syncobj::create(device.as_fd(), false) else {
                continue;
            };
            let timeline = Timeline {
                device,
                handle: created.handle,
            };
            // Timeline support: a query of a fresh syncobj reads point 0.
            if timeline.signaled_point().is_some() {
                return Some(timeline);
            }
        }
        None
    }

    /// An fd for the client to import it by.
    pub fn export(&self) -> OwnedFd {
        let h = drm_ffi::syncobj::handle_to_fd(self.device.as_fd(), self.handle, false).unwrap();
        unsafe { OwnedFd::from_raw_fd(h.fd) }
    }

    /// Signal @p point, as the client's GPU does when its rendering is done.
    pub fn signal(&self, point: u64) {
        drm_ffi::syncobj::timeline_signal(self.device.as_fd(), &[self.handle], &[point]).unwrap();
    }

    /// The last point signaled.
    pub fn signaled_point(&self) -> Option<u64> {
        let mut points = [0u64];
        drm_ffi::syncobj::query(self.device.as_fd(), &[self.handle], &mut points, false).ok()?;
        Some(points[0])
    }
}

impl Drop for Timeline {
    fn drop(&mut self) {
        let _ = drm_ffi::syncobj::destroy(self.device.as_fd(), self.handle);
    }
}
