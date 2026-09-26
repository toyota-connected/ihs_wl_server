// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Format, Fourcc, Modifier};
use smithay::delegate_dmabuf;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::dmabuf::{
    DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier,
};

use crate::caps::{Caps, FormatModifier};
use crate::state::State;

/// The dma-buf formats offered to clients: what the shell imports. A fourcc
/// smithay does not know is left out rather than guessed at.
pub fn formats(offered: &[FormatModifier]) -> Vec<Format> {
    offered
        .iter()
        .filter_map(|f| {
            Fourcc::try_from(f.fourcc).ok().map(|code| Format {
                code,
                modifier: Modifier::from(f.modifier),
            })
        })
        .collect()
}

/// The linux-dmabuf global. With the shell's render device known it is v4,
/// with default feedback naming that device as main_device: a client
/// allocates on the GPU the shell imports on, and Mesa's Wayland EGL -- which
/// finds its device through that feedback (or wl_drm, which is not offered) --
/// renders on the GPU instead of falling back to software. Without it, v3.
/// Returns the global and its default feedback, if any.
pub fn global(
    state: &mut DmabufState,
    dh: &DisplayHandle,
    caps: &Caps,
) -> (DmabufGlobal, Option<DmabufFeedback>) {
    let offered = formats(&caps.formats);
    if let Some(device) = caps.render_device {
        match DmabufFeedbackBuilder::new(device as libc::dev_t, offered.clone()).build() {
            Ok(feedback) => {
                tracing::info!(device, "dma-buf feedback: main device");
                let global = state.create_global_with_default_feedback::<State>(dh, &feedback);
                return (global, Some(feedback));
            }
            Err(e) => tracing::warn!("dma-buf feedback: {e}; offering v3"),
        }
    }
    (state.create_global::<State>(dh, offered), None)
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        _dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        // The module has no GPU to test an import with, and needs none: the
        // global only offers what the shell imports, and the shell is where
        // the buffer is used.
        let _ = notifier.successful::<State>();
    }

    // A surface asking after the shell hinted its layer gets the tranche.
    fn new_surface_feedback(
        &mut self,
        surface: &WlSurface,
        _global: &DmabufGlobal,
    ) -> Option<DmabufFeedback> {
        self.hinted_feedback(surface)
    }
}

delegate_dmabuf!(State);
