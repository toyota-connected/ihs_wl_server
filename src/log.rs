//! Routes `tracing` (ours and smithay's) into `ihs_log`, so the module's
//! records land in whatever sink the shell selected (DLT, console, file).
//!
//! Filter with `IHS_WL_LOG` (EnvFilter syntax, default `info,smithay=warn`).
//! Before the shell has called `ihs_log_start` -- or with no shell at all, as
//! in the tests -- records go to stderr.

use std::ffi::CString;
use std::io::{self, Write};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Once;

use tracing::{Level, Metadata};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

use crate::ffi::ihs::sys;

const TAG: &str = "IHWL";

/// -1 until `ihs_log_context_open` succeeds; retried on every record until it
/// does, because it answers -1 until the shell has run `ihs_log_start`.
static CONTEXT: AtomicI32 = AtomicI32::new(-1);

fn context() -> i32 {
    let ctx = CONTEXT.load(Ordering::Acquire);
    if ctx >= 0 {
        return ctx;
    }
    let tag = CString::new(TAG).unwrap();
    let desc = CString::new("ihs_wl_server embedded Wayland server").unwrap();
    let opts = sys::IhsLogContextOptions {
        struct_size: std::mem::size_of::<sys::IhsLogContextOptions>(),
        description: desc.as_ptr(),
        ..Default::default()
    };
    let ctx = unsafe { sys::ihs_log_context_open(tag.as_ptr(), &opts) };
    if ctx >= 0 {
        CONTEXT.store(ctx, Ordering::Release);
    }
    ctx
}

fn ihs_level(level: &Level) -> u8 {
    (match *level {
        Level::ERROR => sys::IHS_LEVEL_ERROR,
        Level::WARN => sys::IHS_LEVEL_WARN,
        Level::INFO => sys::IHS_LEVEL_INFO,
        Level::DEBUG => sys::IHS_LEVEL_DEBUG,
        Level::TRACE => sys::IHS_LEVEL_VERBOSE,
    }) as u8
}

pub struct Line {
    level: u8,
    buf: Vec<u8>,
}

impl Write for Line {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        let text = self.buf.trim_ascii_end();
        if text.is_empty() {
            return;
        }
        let ctx = context();
        let sent = ctx >= 0
            && unsafe { sys::ihs_log(ctx, self.level, text.as_ptr().cast(), text.len()) } == 0;
        if !sent {
            let _ = writeln!(io::stderr(), "[{TAG}] {}", String::from_utf8_lossy(text));
        }
    }
}

struct MakeLine;

impl<'a> MakeWriter<'a> for MakeLine {
    type Writer = Line;
    fn make_writer(&'a self) -> Line {
        Line {
            level: sys::IHS_LEVEL_INFO as u8,
            buf: Vec::new(),
        }
    }
    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Line {
        Line {
            level: ihs_level(meta.level()),
            buf: Vec::new(),
        }
    }
}

/// Install the subscriber once per process. The cdylib carries its own
/// `tracing` statics, so this never collides with another Rust module's.
pub fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let filter = EnvFilter::try_from_env("IHS_WL_LOG")
            .unwrap_or_else(|_| EnvFilter::new("info,smithay=warn"));
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(MakeLine)
            .with_ansi(false)
            .without_time()
            .with_level(false)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}
