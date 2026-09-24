use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Format, Fourcc, Modifier};
use smithay::delegate_dmabuf;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};

use crate::caps::FormatModifier;
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
}

delegate_dmabuf!(State);
