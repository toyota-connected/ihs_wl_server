// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! The render nodes the server opens: for explicit sync and for staging
//! shared-memory buffers. Render nodes only -- opening a primary node could
//! contend with the shell for DRM master.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

/// Render nodes to try, in order: the one @p override_var names alone when
/// set, else every `/dev/dri/renderD*`.
fn candidates(override_var: &str) -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os(override_var) {
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

/// Each candidate render node, opened read-write, in order.
pub fn render_nodes(override_var: &str) -> impl Iterator<Item = (PathBuf, File)> {
    candidates(override_var).into_iter().filter_map(|path| {
        if !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("renderD"))
        {
            tracing::warn!(?path, "not a render node; skipped");
            return None;
        }
        match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&path)
        {
            Ok(file) => Some((path, file)),
            Err(e) => {
                tracing::debug!(?path, "open: {e}");
                None
            }
        }
    })
}
