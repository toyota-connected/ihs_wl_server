// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

//! gbm_buffer_backend: offered only with something to import with; a
//! buffer shared over it is shown as a dma-buf with the layout its metadata
//! gives, y-inverted when flagged; a failed import, or a second create on
//! one params object, is refused.

mod common;

use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Arc, Mutex};

use common::client::XRGB8888;
use common::harness::Harness;
use common::{mock_host, serial};
use ihs_wl_server::gbm_buffer::{set_test_import, Import, Plane, Planes};

const APP: &str = "org.example.gbm";
/// drm_fourcc.h DRM_FORMAT_MOD_QCOM_COMPRESSED.
const QCOM_COMPRESSED: u64 = (0x05 << 56) | 1;
/// DRM_FORMAT_NV12.
const NV12: u32 = 0x3231_564e;
/// wl_output.transform flipped_180.
const FLIPPED_180: u32 = 6;

/// Reads the layout from the metadata: "compressed" means compressed. Records
/// every call.
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<(u32, u32, u32, String)>>,
}

impl Import for Fake {
    fn layout(
        &self,
        fd: &OwnedFd,
        metadata: OwnedFd,
        width: u32,
        height: u32,
        format: u32,
    ) -> Result<Planes, String> {
        assert!(fd.as_raw_fd() >= 0);
        let mut meta = String::new();
        std::fs::File::from(metadata)
            .read_to_string(&mut meta)
            .unwrap();
        let meta = meta.trim_end_matches('\0').to_owned();
        self.calls
            .lock()
            .unwrap()
            .push((width, height, format, meta.clone()));
        if meta == "bad" {
            return Err("no".into());
        }
        let modifier = if meta.starts_with("compressed") {
            QCOM_COMPRESSED
        } else {
            0
        };
        // Two planes for NV12: luma, then interleaved chroma below it.
        let planes = if format == NV12 {
            let stride = width.next_multiple_of(128);
            vec![
                Plane { offset: 0, stride },
                Plane {
                    offset: stride * height.next_multiple_of(32),
                    stride,
                },
            ]
        } else {
            vec![Plane {
                offset: 0,
                stride: width * 4 + 64,
            }]
        };
        Ok(Planes { modifier, planes })
    }
}

/// Uninstalls the fake even if the test fails.
struct Installed(Arc<Fake>);

impl Installed {
    fn new() -> Self {
        let fake = Arc::new(Fake::default());
        set_test_import(Some(fake.clone()));
        Installed(fake)
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        set_test_import(None);
    }
}

#[test]
fn not_offered_without_an_importer() {
    let _serial = serial();
    // Mesa's libgbm, here, cannot import gbm buffers.
    let h = Harness::new("ihs-wl-test-gbm-none");
    assert!(!h.client.globals().iter().any(|g| g == "gbm_buffer_backend"));
}

#[test]
fn a_gbm_buffer_is_shown_as_a_dma_buf() {
    let _serial = serial();
    let fake = Installed::new();
    let mut h = Harness::new("ihs-wl-test-gbm");
    assert!(h.client.globals().iter().any(|g| g == "gbm_buffer_backend"));
    h.client.create_toplevel(APP, "gbm");
    let buf = h
        .client
        .gbm_buffer(64, 32, XRGB8888, 0, b"compressed")
        .expect("created");
    assert_eq!(
        fake.0.calls.lock().unwrap().as_slice(),
        &[(64, 32, XRGB8888, "compressed".to_owned())],
        "imported with the client's size, format and metadata"
    );
    h.client.commit_buffer(buf, false);
    h.bind(1, APP, 640.0, 480.0);

    let subs = h.wait_submissions(1, 1);
    let l = &subs.last().unwrap().layers[0];
    assert_eq!((l.width, l.height, l.fourcc), (64, 32, XRGB8888));
    assert_eq!(l.modifier, QCOM_COMPRESSED, "as the metadata says");
    assert_eq!(l.stride, 64 * 4 + 64, "as the metadata says");
    assert_eq!(l.transform, 0);
    assert_eq!(l.dst, (0, 0, 64, 32));

    // Released and retired like any dma-buf.
    h.client.destroy_buffer(buf);
    h.rec
        .wait_for("retired", |_| !mock_host::retired().is_empty());
    mock_host::dispose_view(1);
}

#[test]
fn a_two_plane_gbm_buffer_goes_with_both_planes() {
    let _serial = serial();
    let _fake = Installed::new();
    let mut h = Harness::new("ihs-wl-test-gbm-nv12");
    h.client.create_toplevel(APP, "gbm");
    let buf = h
        .client
        .gbm_buffer(640, 368, NV12, 0, b"compressed-video")
        .expect("created");
    h.client.commit_buffer(buf, false);
    h.bind(1, APP, 640.0, 480.0);

    let subs = h.wait_submissions(1, 1);
    let l = &subs.last().unwrap().layers[0];
    assert_eq!((l.width, l.height, l.fourcc), (640, 368, NV12));
    assert_eq!(l.plane_count, 2);
    assert_eq!(l.modifier, QCOM_COMPRESSED);
    assert_eq!(l.planes, vec![(0, 640), (640 * 384, 640)]);
    assert!(l.one_fd, "planes of one dma-buf go as one fd");
    mock_host::dispose_view(1);
}

#[test]
fn a_y_inverted_gbm_buffer_is_flipped_back() {
    let _serial = serial();
    let _fake = Installed::new();
    let mut h = Harness::new("ihs-wl-test-gbm-invert");
    h.client.create_toplevel(APP, "gbm");
    let buf = h
        .client
        .gbm_buffer(64, 32, XRGB8888, 1, b"linear")
        .expect("created");
    h.client.commit_buffer(buf, false);
    h.bind(1, APP, 640.0, 480.0);

    let subs = h.wait_submissions(1, 1);
    let l = &subs.last().unwrap().layers[0];
    assert_eq!(l.modifier, 0);
    assert_eq!(l.transform, FLIPPED_180);
    assert_eq!(l.src, (0, 0, 64 << 16, 32 << 16));
    assert_eq!(l.dst, (0, 0, 64, 32));
    mock_host::dispose_view(1);
}

#[test]
fn a_failed_import_or_a_reused_params_is_refused() {
    let _serial = serial();
    let _fake = Installed::new();
    let mut h = Harness::new("ihs-wl-test-gbm-fail");
    assert_eq!(h.client.gbm_buffer(64, 32, XRGB8888, 0, b"bad"), None);
    // Not a DRM format.
    assert_eq!(h.client.gbm_buffer(64, 32, 0x1234_5678, 0, b"linear"), None);
    assert!(h.client.gbm_params_reused_fails());
}
