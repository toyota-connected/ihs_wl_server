//! A client's surface tree reaches its view as layers: binding by app_id,
//! the layer list and its geometry, buffer ids and their retirement, and
//! buffers going back to the client once the shell is done with them.

mod common;

use common::client::{Client, XRGB8888};
use common::{mock_host, serial, Recorder};
use ihs_wl_server::observe::Observed;

/// creationParams as the widget's StandardMessageCodec encodes
/// `{'app_id': app_id}`.
fn params(app_id: &str) -> Vec<u8> {
    let mut b = vec![13u8, 1]; // map, 1 entry
    for s in ["app_id", app_id] {
        b.push(7); // string
        b.push(s.len() as u8);
        b.extend_from_slice(s.as_bytes());
    }
    b
}

struct Harness {
    rec: Recorder,
    client: Client,
}

impl Harness {
    fn new(socket: &str) -> Self {
        mock_host::install();
        let rec = Recorder::install();
        ihs_wl_server::start(ihs_wl_server::Config {
            socket_name: Some(socket.into()),
        })
        .unwrap();
        let client = Client::connect(socket);
        Harness { rec, client }
    }

    /// The submissions for @p view since @p after (a count of submissions).
    fn submissions(view: i32) -> Vec<mock_host::Submission> {
        mock_host::submissions()
            .into_iter()
            .filter(|s| s.view_id == view)
            .collect()
    }

    fn wait_submissions(&self, view: i32, n: usize) -> Vec<mock_host::Submission> {
        self.rec
            .wait_for("submission", |_| Self::submissions(view).len() >= n);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let s = Self::submissions(view);
            if s.len() >= n {
                return s;
            }
            assert!(std::time::Instant::now() < deadline, "no submission {n}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        ihs_wl_server::stop().unwrap();
        mock_host::uninstall();
        common::clear_observer();
    }
}

#[test]
fn a_toplevel_reaches_its_view_as_a_layer() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-layer");
    h.client.create_toplevel("org.example.a", "a");
    let buf = h.client.new_dmabuf(100, 80);
    h.client.set_opaque(100, 80);
    h.client.commit_buffer(buf, false);

    // A view for another app does not take it.
    assert_eq!(
        mock_host::create_view_with_params(
            "ihs_wl/toplevel",
            3,
            640.0,
            480.0,
            &params("org.example.other")
        ),
        0
    );
    assert_eq!(
        mock_host::create_view_with_params(
            "ihs_wl/toplevel",
            4,
            640.0,
            480.0,
            &params("org.example.a")
        ),
        0
    );
    h.rec.wait_for("ViewBound", |e| {
        matches!(e, Observed::ViewBound { view_id: 4, .. })
    });
    assert!(
        !h.rec
            .events()
            .iter()
            .any(|e| matches!(e, Observed::ViewBound { view_id: 3, .. })),
        "a view bound a toplevel with a different app_id"
    );

    let subs = h.wait_submissions(4, 1);
    let layers = &subs[0].layers;
    assert_eq!(layers.len(), 1);
    let l = &layers[0];
    assert_ne!(l.buffer_id, 0);
    assert_eq!(
        (l.width, l.height, l.fourcc, l.plane_count),
        (100, 80, XRGB8888, 1)
    );
    assert_eq!(l.src, (0, 0, 100 << 16, 80 << 16));
    assert_eq!(l.dst, (0, 0, 100, 80));
    assert_eq!(l.transform, 0);
    assert!(l.opaque, "the opaque region covers the surface");

    // The view is laid out: the toplevel is configured to its size.
    mock_host::resize_view(4, 640.0, 480.0);
    h.client
        .dispatch_until("configure to the view's size", |c| {
            c.configure_size() == Some((640, 480))
        });
    mock_host::dispose_view(3);
    mock_host::dispose_view(4);
}

#[test]
fn buffer_ids_are_stable_and_retired_with_the_buffer() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-ids");
    h.client.create_toplevel("org.example.b", "b");
    let a = h.client.new_dmabuf(64, 64);
    let b = h.client.new_dmabuf(64, 64);
    h.client.commit_buffer(a, false);
    assert_eq!(
        mock_host::create_view_with_params(
            "ihs_wl/toplevel",
            5,
            64.0,
            64.0,
            &params("org.example.b")
        ),
        0
    );
    h.wait_submissions(5, 1);
    h.client.commit_buffer(b, false);
    h.client.commit_buffer(a, false);
    let subs = h.wait_submissions(5, 3);
    let ids: Vec<u32> = subs.iter().map(|s| s.layers[0].buffer_id).collect();
    assert_eq!(ids[0], ids[2], "a buffer shown again kept its id");
    assert_ne!(ids[0], ids[1], "two buffers share an id");
    let seqs: Vec<u64> = subs.iter().map(|s| s.seq).collect();
    assert!(
        seqs.windows(2).all(|w| w[1] > w[0]),
        "seq must increase: {seqs:?}"
    );

    h.client.destroy_buffer(b);
    h.rec.wait_for("Retired", |e| {
        *e == Observed::Retired {
            view_id: 5,
            buffer_id: ids[1],
        }
    });
    assert!(mock_host::retired().contains(&(5, ids[1])));
    assert!(!mock_host::retired().contains(&(5, ids[0])));
    mock_host::dispose_view(5);
}

/// With no release fence (the Vulkan backends), a buffer goes back once a
/// later frame is on screen; frame callbacks fire as frames are shown.
#[test]
fn buffers_go_back_once_a_later_frame_is_shown() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-release");
    h.client.create_toplevel("org.example.c", "c");
    let a = h.client.new_dmabuf(32, 32);
    let b = h.client.new_dmabuf(32, 32);
    assert_eq!(
        mock_host::create_view_with_params(
            "ihs_wl/toplevel",
            6,
            32.0,
            32.0,
            &params("org.example.c")
        ),
        0
    );
    h.client.commit_buffer(a, true);
    let first = h.wait_submissions(6, 1)[0].seq;
    h.client.commit_buffer(b, true);
    let second = h.wait_submissions(6, 2)[1].seq;
    assert_eq!(h.client.released(a), 0, "released while the shell holds it");

    // The first frame shown: a is still current in it, nothing goes back,
    // but the clients may draw on.
    mock_host::present(6, first);
    h.client
        .dispatch_until("frame callback", |c| c.frames_done() >= 1);
    assert_eq!(h.client.released(a), 0);

    // The frame that replaced it is shown: a is free.
    mock_host::present(6, second);
    h.client
        .dispatch_until("a released", |c| c.released(a) == 1);
    assert_eq!(h.client.released(b), 0, "the buffer on screen went back");
    mock_host::dispose_view(6);
    // With the view gone nothing but the surface holds b, and a surface keeps
    // its attached buffer until the client replaces it -- which hands it
    // back at once, no frame to wait for.
    h.client.roundtrip();
    assert_eq!(h.client.released(b), 0, "released while still attached");
    h.client.commit_buffer(a, false);
    h.client
        .dispatch_until("b released once replaced", |c| c.released(b) == 1);
}

/// With a release fence per layer (the EGL backends), a buffer goes back
/// when its fence signals, and only then.
#[test]
fn buffers_go_back_when_their_fence_signals() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-fence");
    mock_host::set_fenced(true);
    h.client.create_toplevel("org.example.d", "d");
    let a = h.client.new_dmabuf(32, 32);
    let b = h.client.new_dmabuf(32, 32);
    assert_eq!(
        mock_host::create_view_with_params(
            "ihs_wl/toplevel",
            7,
            32.0,
            32.0,
            &params("org.example.d")
        ),
        0
    );
    h.client.commit_buffer(a, false);
    let id_a = h.wait_submissions(7, 1)[0].layers[0].buffer_id;
    h.client.commit_buffer(b, false);
    h.wait_submissions(7, 2);
    // Presenting does not matter here: the fence decides.
    mock_host::present(7, 2);
    h.client.roundtrip();
    assert_eq!(h.client.released(a), 0);
    mock_host::signal_release(id_a);
    h.client
        .dispatch_until("a released by its fence", |c| c.released(a) == 1);
    mock_host::dispose_view(7);
    mock_host::set_fenced(false);
}

/// A subsurface is a layer of its own above its parent, at its position,
/// and keeps its layer id as the tree is committed again.
#[test]
fn a_subsurface_is_a_layer_above_its_parent() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-sub");
    h.client.create_toplevel("org.example.e", "e");
    let base = h.client.new_dmabuf(200, 100);
    let video = h.client.new_dmabuf(80, 40);
    h.client.commit_buffer(base, false);
    assert_eq!(
        mock_host::create_view_with_params(
            "ihs_wl/toplevel",
            8,
            200.0,
            100.0,
            &params("org.example.e")
        ),
        0
    );
    h.wait_submissions(8, 1);
    h.client.add_subsurface(video, 10, 20);
    let n = h.wait_submissions(8, 2).len();
    // The last submission has both layers.
    h.rec.wait_for("two layers", |_| {
        Harness::submissions(8)
            .last()
            .is_some_and(|s| s.layers.len() == 2)
    });
    let last = Harness::submissions(8).pop().unwrap();
    assert!(n >= 2);
    let (bottom, top) = (&last.layers[0], &last.layers[1]);
    assert_eq!(
        (bottom.width, bottom.height),
        (200, 100),
        "the parent is at the bottom"
    );
    assert_eq!(top.dst, (10, 20, 80, 40));
    assert_ne!(bottom.layer_id, top.layer_id);

    // Committed again, each layer keeps its id.
    h.client.commit_buffer(base, false);
    h.rec.wait_for("a later submission", |_| {
        Harness::submissions(8)
            .last()
            .is_some_and(|s| s.seq > last.seq)
    });
    let again = Harness::submissions(8).pop().unwrap();
    assert_eq!(again.layers[0].layer_id, bottom.layer_id);
    assert_eq!(again.layers[1].layer_id, top.layer_id);
    mock_host::dispose_view(8);
}

/// A client that exits takes its buffers with it: each is retired in every
/// view it was shown in.
#[test]
fn a_client_that_exits_has_its_buffers_retired() {
    let _serial = serial();
    let mut h = Harness::new("ihs-wl-test-exit");
    h.client.create_toplevel("org.example.f", "f");
    let a = h.client.new_dmabuf(16, 16);
    let b = h.client.new_dmabuf(16, 16);
    h.client.commit_buffer(a, false);
    assert_eq!(
        mock_host::create_view_with_params(
            "ihs_wl/toplevel",
            9,
            16.0,
            16.0,
            &params("org.example.f")
        ),
        0
    );
    h.wait_submissions(9, 1);
    h.client.commit_buffer(b, false);
    let subs = h.wait_submissions(9, 2);
    let (id_a, id_b) = (subs[0].layers[0].buffer_id, subs[1].layers[0].buffer_id);

    // Replace the client with a fresh connection so the harness can stop.
    h.client = Client::connect("ihs-wl-test-exit");
    for id in [id_a, id_b] {
        h.rec.wait_for("Retired", |e| {
            *e == Observed::Retired {
                view_id: 9,
                buffer_id: id,
            }
        });
    }
    mock_host::dispose_view(9);
}
