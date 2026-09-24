//! Buffer identity: a `buffer_id` per `wl_buffer`, the key the shell caches
//! its import of the buffer under.
//!
//! An id is handed out the first time a buffer is submitted and never reused,
//! so a client's ring of two to four buffers hits the shell's import cache on
//! every frame. When the client destroys the buffer, every view it was shown
//! in is told to drop its import (ihs_pv_retire_buffer).

use std::collections::HashMap;

use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::Resource;

struct Entry {
    id: u32,
    /// Views the buffer was submitted to.
    views: Vec<i32>,
}

#[derive(Default)]
pub struct BufferIds {
    last: u32,
    by_buffer: HashMap<ObjectId, Entry>,
}

impl BufferIds {
    /// The buffer's id, handed out now if it has none, and noted as shown in
    /// @p view_id.
    pub fn id_for(&mut self, buffer: &WlBuffer, view_id: i32) -> u32 {
        let last = &mut self.last;
        let entry = self.by_buffer.entry(buffer.id()).or_insert_with(|| {
            // 0 is left unused so a zeroed frame never aliases a real buffer.
            // Wrapping takes four billion buffers; the shell drops an import
            // it no longer needs on its own bound, so a reused id at worst
            // re-imports.
            *last = last.wrapping_add(1).max(1);
            Entry {
                id: *last,
                views: Vec::new(),
            }
        });
        if !entry.views.contains(&view_id) {
            entry.views.push(view_id);
        }
        entry.id
    }

    /// The buffer is gone: its id and the views to retire it in.
    pub fn destroyed(&mut self, buffer: &WlBuffer) -> Option<(u32, Vec<i32>)> {
        self.by_buffer.remove(&buffer.id()).map(|e| (e.id, e.views))
    }

    /// A view is gone: nothing to retire there any more.
    pub fn forget_view(&mut self, view_id: i32) {
        for entry in self.by_buffer.values_mut() {
            entry.views.retain(|&v| v != view_id);
        }
    }
}
