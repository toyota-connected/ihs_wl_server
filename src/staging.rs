// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! Shared-memory surfaces, shown through dma-bufs the server owns.
//!
//! The shell imports dma-bufs; a `wl_shm` buffer is plain memory. Each shm
//! surface gets a small ring of linear dma-bufs. They come from a contiguous
//! (CMA) dma-heap when there is one -- a display that can only scan out
//! contiguous memory, as on the Raspberry Pi 5, takes them onto a plane --
//! else from gbm on a render node. A commit copies what changed since a free slot was last
//! filled -- smithay tracks buffer damage per commit -- and that
//! slot goes to the shell in the client buffer's place. The client's buffer is
//! only read during the copy.
//!
//! A slot is busy while the shell may read it: the ring keeps one `Arc` of
//! its hold, and the view's holds (submit.rs) keep another until the shell is
//! done. With every slot busy the ring grows, up to MAX_SLOTS; past that the
//! newest slot is shown again unchanged and the copy waits for the next
//! commit, whose damage then covers what this one missed.

use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex};

use smithay::backend::allocator::dmabuf::{AsDmabuf, Dmabuf, DmabufFlags, DmabufSyncFlags};
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBuffer, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::Buffer as _;
use smithay::backend::allocator::{Allocator, Fourcc, Modifier};
use smithay::backend::renderer::utils::{CommitCounter, RendererSurfaceState};
use smithay::reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm};
use smithay::utils::DeviceFd;
use smithay::wayland::compositor::SurfaceData;
use smithay::wayland::shm::{self, BufferData};

/// Slots per surface, at most: double buffering, plus the shell's pipeline.
const MAX_SLOTS: usize = 4;

/// Where staging buffers come from.
enum Backend {
    /// A dma-heap under /dev/dma_heap.
    Heap(File),
    Gbm(GbmAllocator<DeviceFd>),
}

/// The backend, found on first use.
#[derive(Default)]
pub struct Stager {
    backend: Option<Backend>,
    tried: bool,
    next_uid: u64,
}

impl Stager {
    fn backend(&mut self) -> Option<&mut Backend> {
        if !self.tried {
            self.tried = true;
            self.backend = open_backend();
        }
        self.backend.as_mut()
    }
}

/// `IHS_WL_STAGING_DEVICE` names a render node for gbm, alone. Otherwise the
/// heap `IHS_WL_STAGING_HEAP` names, else the first contiguous one, else gbm on
/// the first render node that allocates.
fn open_backend() -> Option<Backend> {
    if std::env::var_os("IHS_WL_STAGING_DEVICE").is_none() {
        if let Some(heap) = open_heap() {
            return Some(Backend::Heap(heap));
        }
    }
    for (path, file) in crate::nodes::render_nodes("IHS_WL_STAGING_DEVICE") {
        let fd = DeviceFd::from(OwnedFd::from(file));
        match GbmDevice::new(fd) {
            Ok(device) => {
                tracing::info!(?path, "shared-memory surfaces staged with gbm on");
                return Some(Backend::Gbm(GbmAllocator::new(
                    device,
                    GbmBufferFlags::LINEAR,
                )));
            }
            Err(e) => tracing::debug!(?path, "gbm: {e}"),
        }
    }
    tracing::warn!("nothing to stage shared-memory surfaces with; they are not shown");
    None
}

fn open_heap() -> Option<File> {
    let dir = std::path::Path::new("/dev/dma_heap");
    let name = match std::env::var("IHS_WL_STAGING_HEAP") {
        Ok(name) => name,
        Err(_) => {
            // Contiguous heaps are named after their reserved-memory node:
            // "linux,cma" generically, other "...cma..." or "reserved".
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .ok()?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains("cma") || n == "reserved")
                .collect();
            names.sort_by_key(|n| (n != "linux,cma", n.clone()));
            names.into_iter().next()?
        }
    };
    let path = dir.join(&name);
    match std::fs::OpenOptions::new().read(true).open(&path) {
        Ok(heap) => {
            tracing::info!(?path, "shared-memory surfaces staged in dma-heap");
            Some(heap)
        }
        Err(e) => {
            tracing::debug!(?path, "dma-heap: {e}");
            None
        }
    }
}

/// A slot's memory, and how the CPU writes it.
enum Mem {
    /// Mapped once, for the slot's life.
    Heap {
        map: *mut u8,
        len: usize,
        stride: usize,
    },
    /// Mapped for each copy: gbm rather than the dma-buf's own mmap, which
    /// some drivers refuse (v3d).
    Gbm(GbmBuffer),
}

impl Drop for Mem {
    fn drop(&mut self) {
        if let Mem::Heap { map, len, .. } = *self {
            // SAFETY: mapped in alloc_heap, unmapped only here.
            unsafe { libc::munmap(map.cast(), len) };
        }
    }
}

struct Slot {
    uid: u64,
    mem: Mem,
    dmabuf: Dmabuf,
    /// The commit whose content the slot holds.
    commit: Option<CommitCounter>,
    /// Busy while another clone lives.
    hold: Arc<()>,
}

// SAFETY: a heap slot's mapping is only written on the compositor thread,
// under its ring's lock; Send is what lets the ring sit in the surface's data
// map.
unsafe impl Send for Slot {}

#[derive(Default)]
struct Ring {
    width: i32,
    height: i32,
    fourcc: Option<Fourcc>,
    slots: Vec<Slot>,
    /// Index of the slot last filled.
    newest: Option<usize>,
}

/// A surface's staging ring, in its data map.
#[derive(Default)]
pub struct StagingRing(Mutex<Ring>);

/// A slot to show: its dma-buf, the hold that keeps it from reuse, and its
/// uid (what its buffer id is keyed by).
pub struct Staged {
    pub dmabuf: Dmabuf,
    pub hold: Arc<()>,
    pub uid: u64,
}

fn fourcc(format: wl_shm::Format) -> Option<Fourcc> {
    match format {
        wl_shm::Format::Argb8888 => Some(Fourcc::Argb8888),
        wl_shm::Format::Xrgb8888 => Some(Fourcc::Xrgb8888),
        _ => None,
    }
}

/// Stage the surface's current shm buffer. None when it cannot be shown (no
/// allocator, a format other than ARGB/XRGB8888, a failed allocation);
/// @p retired collects the uids of slots dropped, whose buffer ids the caller
/// retires.
pub fn stage(
    stager: &mut Stager,
    states: &SurfaceData,
    buffer: &WlBuffer,
    render: &RendererSurfaceState,
    retired: &mut Vec<u64>,
) -> Option<Staged> {
    let data = shm::with_buffer_contents(buffer, |_, _, data| data).ok()?;
    let fourcc = fourcc(data.format)?;
    states
        .data_map
        .insert_if_missing_threadsafe(StagingRing::default);
    let mut ring = states
        .data_map
        .get::<StagingRing>()
        .unwrap()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    if ring.width != data.width || ring.height != data.height || ring.fourcc != Some(fourcc) {
        retired.extend(ring.slots.drain(..).map(|s| s.uid));
        *ring = Ring {
            width: data.width,
            height: data.height,
            fourcc: Some(fourcc),
            ..Ring::default()
        };
    }

    // Unchanged since the newest slot was filled (another surface of the tree
    // committed, or the view rebound): show that slot again, no copy.
    if let Some(newest) = ring.newest {
        let slot = &ring.slots[newest];
        if slot.commit == Some(render.current_commit()) {
            return Some(Staged {
                dmabuf: slot.dmabuf.clone(),
                hold: slot.hold.clone(),
                uid: slot.uid,
            });
        }
    }

    let free = ring
        .slots
        .iter()
        .position(|s| Arc::strong_count(&s.hold) == 1);
    let index = match free {
        Some(i) => i,
        None if ring.slots.len() < MAX_SLOTS => {
            let slot = allocate(stager, &data, fourcc)?;
            ring.slots.push(slot);
            ring.slots.len() - 1
        }
        None => {
            // Every slot is on screen or queued: show the newest again.
            let slot = &ring.slots[ring.newest?];
            return Some(Staged {
                dmabuf: slot.dmabuf.clone(),
                hold: slot.hold.clone(),
                uid: slot.uid,
            });
        }
    };

    let slot = &mut ring.slots[index];
    copy_damage(slot, buffer, &data, render)?;
    slot.commit = Some(render.current_commit());
    let staged = Staged {
        dmabuf: slot.dmabuf.clone(),
        hold: slot.hold.clone(),
        uid: slot.uid,
    };
    ring.newest = Some(index);
    Some(staged)
}

/// The slot uids of @p states' ring, which is dropped: the surface is gone.
pub fn drop_ring(states: &SurfaceData) -> Vec<u64> {
    let Some(ring) = states.data_map.get::<StagingRing>() else {
        return Vec::new();
    };
    let mut ring = ring.0.lock().unwrap_or_else(|e| e.into_inner());
    let uids = ring.slots.drain(..).map(|s| s.uid).collect();
    *ring = Ring::default();
    uids
}

fn allocate(stager: &mut Stager, data: &BufferData, fourcc: Fourcc) -> Option<Slot> {
    stager.next_uid += 1;
    let uid = stager.next_uid;
    let (w, h) = (data.width.max(1) as u32, data.height.max(1) as u32);
    let (mem, dmabuf) = match stager.backend()? {
        Backend::Heap(heap) => alloc_heap(heap, w, h, fourcc)?,
        Backend::Gbm(allocator) => {
            let bo = allocator
                .create_buffer(w, h, fourcc, &[Modifier::Linear])
                .map_err(|e| tracing::warn!("staging buffer {w}x{h}: {e}"))
                .ok()?;
            let exported = bo
                .export()
                .map_err(|e| tracing::warn!("staging buffer export: {e}"))
                .ok()?;
            let dmabuf = explicitly_linear(exported, fourcc)?;
            (Mem::Gbm(bo), dmabuf)
        }
    };
    Some(Slot {
        uid,
        mem,
        dmabuf,
        commit: None,
        hold: Arc::new(()),
    })
}

/// A linear @p w x @p h buffer from @p heap, mapped.
fn alloc_heap(heap: &File, w: u32, h: u32, fourcc: Fourcc) -> Option<(Mem, Dmabuf)> {
    #[repr(C)]
    struct Alloc {
        len: u64,
        fd: u32,
        fd_flags: u32,
        heap_flags: u64,
    }
    // _IOWR('H', 0, struct dma_heap_allocation_data)
    const DMA_HEAP_IOCTL_ALLOC: libc::c_ulong = 0xC018_4800;
    // A pitch every importer we know of takes.
    let stride = (w as usize * 4).next_multiple_of(64);
    let len = stride * h as usize;
    let mut req = Alloc {
        len: len as u64,
        fd: 0,
        fd_flags: (libc::O_RDWR | libc::O_CLOEXEC) as u32,
        heap_flags: 0,
    };
    // SAFETY: a valid heap fd and a request of the kernel's layout.
    if unsafe { libc::ioctl(heap.as_raw_fd(), DMA_HEAP_IOCTL_ALLOC, &mut req) } != 0 {
        tracing::warn!(
            "staging buffer {w}x{h} from dma-heap: {}",
            std::io::Error::last_os_error()
        );
        return None;
    }
    // SAFETY: the kernel handed us this fd.
    let fd = unsafe { OwnedFd::from_raw_fd(req.fd as i32) };
    // SAFETY: mapping a dma-buf fd we own, of the length we allocated.
    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if map == libc::MAP_FAILED {
        tracing::warn!("staging buffer map: {}", std::io::Error::last_os_error());
        return None;
    }
    let mem = Mem::Heap {
        map: map.cast(),
        len,
        stride,
    };
    let mut builder = Dmabuf::builder(
        (w as i32, h as i32),
        fourcc,
        Modifier::Linear,
        DmabufFlags::empty(),
    );
    builder.add_plane(fd, 0, 0, stride as u32);
    Some((mem, builder.build()?))
}

/// A buffer allocated linear can come back from gbm with an unknown
/// modifier (it was allocated by usage, not by modifier). Say what it is, so
/// the shell imports and scans it out as linear rather than guessing.
fn explicitly_linear(dmabuf: Dmabuf, fourcc: Fourcc) -> Option<Dmabuf> {
    if dmabuf.format().modifier != Modifier::Invalid {
        return Some(dmabuf);
    }
    let mut builder = Dmabuf::builder(
        dmabuf.size(),
        fourcc,
        Modifier::Linear,
        DmabufFlags::empty(),
    );
    for (i, ((fd, offset), stride)) in dmabuf
        .handles()
        .zip(dmabuf.offsets())
        .zip(dmabuf.strides())
        .enumerate()
    {
        let fd = fd
            .try_clone_to_owned()
            .map_err(|e| tracing::warn!("staging buffer dup: {e}"))
            .ok()?;
        builder.add_plane(fd, i as u32, offset, stride);
    }
    builder.build()
}

/// Copy what changed since @p slot was filled from the client's buffer.
fn copy_damage(
    slot: &mut Slot,
    buffer: &WlBuffer,
    data: &BufferData,
    render: &RendererSurfaceState,
) -> Option<()> {
    const BPP: usize = 4;
    let damage = render.damage_since(slot.commit);
    let (w, h) = (data.width.max(0) as usize, data.height.max(0) as usize);
    let src_stride = data.stride.max(0) as usize;
    let offset = data.offset.max(0) as usize;

    let copy = |dst: &mut [u8], dst_stride: usize| {
        shm::with_buffer_contents(buffer, |src, src_len, _| {
            // SAFETY: the pool mapping is live for the closure and src_len
            // long; every read below is checked against it.
            let src = unsafe { std::slice::from_raw_parts(src, src_len) };
            for rect in damage.iter() {
                let x0 = (rect.loc.x.max(0) as usize).min(w);
                let y0 = (rect.loc.y.max(0) as usize).min(h);
                let x1 = ((rect.loc.x + rect.size.w).max(0) as usize).min(w);
                let y1 = ((rect.loc.y + rect.size.h).max(0) as usize).min(h);
                if x1 <= x0 {
                    continue;
                }
                let bytes = (x1 - x0) * BPP;
                for row in y0..y1 {
                    let s = offset + row * src_stride + x0 * BPP;
                    let d = row * dst_stride + x0 * BPP;
                    // A pool shrunk under us, or a buffer lying about its
                    // size: stop rather than read or write out of bounds.
                    let (Some(from), Some(to)) = (src.get(s..s + bytes), dst.get_mut(d..d + bytes))
                    else {
                        return;
                    };
                    to.copy_from_slice(from);
                }
            }
        })
        .ok()
    };
    let mapped = match &mut slot.mem {
        Mem::Heap { map, len, stride } => {
            let sync = |flags: DmabufSyncFlags| slot.dmabuf.sync_plane(0, flags).ok();
            sync(DmabufSyncFlags::START | DmabufSyncFlags::WRITE);
            // SAFETY: our mapping of *len bytes, alive as long as the slot.
            let dst = unsafe { std::slice::from_raw_parts_mut(*map, *len) };
            let copied = copy(dst, *stride);
            sync(DmabufSyncFlags::END | DmabufSyncFlags::WRITE);
            Ok(copied)
        }
        Mem::Gbm(bo) => bo.map_mut(0, 0, w as u32, h as u32, |map| {
            let stride = map.stride() as usize;
            copy(map.buffer_mut(), stride)
        }),
    };
    match mapped {
        Ok(copied) => copied,
        Err(e) => {
            tracing::warn!("staging buffer map: {e}");
            None
        }
    }
}
