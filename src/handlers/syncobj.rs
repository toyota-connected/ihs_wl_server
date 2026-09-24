// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

use smithay::delegate_drm_syncobj;
use smithay::wayland::drm_syncobj::{DrmSyncobjHandler, DrmSyncobjState};

use crate::state::State;

// Acquire points hold a commit in the readiness gate (compositor.rs); a
// release point is signaled when the last hold on its buffer is dropped.
impl DrmSyncobjHandler for State {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.syncobj_state.as_mut()
    }
}

delegate_drm_syncobj!(State);
