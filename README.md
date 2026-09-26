# ihs_wl_server

An embedded Wayland server for
[ivi-homescreen](https://github.com/toyota-connected/ivi-homescreen). It lets
a Flutter app host ordinary Wayland clients as platform views: each client
toplevel is shown inside a widget and composed with the rest of the Flutter
scene.

The server is built on [smithay](https://github.com/Smithay/smithay). It is a
shared library that the Flutter app loads over Dart FFI into the shell's
process. The shell remains the only compositor. This module never renders and
has no GPU context of its own. It hands each client buffer, as a dma-buf, to
the shell's platform-view layer interface. The shell then composites it like
any Flutter layer or puts it directly on a display plane.

## Status

Early development. Working today:

- A Wayland socket with `wl_compositor`, `wl_subcompositor`, `xdg_wm_base`,
  `wl_shm`, `wp_single_pixel_buffer_manager_v1`, `zwp_linux_dmabuf_v1`,
  `wp_viewporter`, `wp_presentation`,
  `wp_fifo_manager_v1`, `wp_commit_timing_manager_v1`,
  `wp_linux_drm_syncobj_manager_v1` (when a render node supports it),
  `wl_seat`, `wp_cursor_shape_manager_v1`, `xdg_activation_v1`,
  `wl_data_device_manager`, `wl_output` and `zxdg_output_manager_v1`.
- A view binds to a toplevel by activation token or app_id and configures
  it to the view's size. `xdg_activation_v1` carries the token: a client
  started with one the module issued (`ihs_wl_activation_token`) in
  `XDG_ACTIVATION_TOKEN` activates its toplevel with it, as GTK and Qt do.
- A toplevel's surface tree (the root and its subsurfaces, with viewports,
  buffer transforms and opaque regions) reaches the shell as one layer per
  surface, bottom to top. Each layer keeps a stable id, so it keeps its
  display plane while the tree changes.
- `wl_shm` surfaces (ARGB8888 and XRGB8888) are copied into linear dma-bufs
  the server owns; only what changed since a staging buffer was last filled
  is copied. They come from a contiguous (CMA) dma-heap when there is one, so
  a display that scans out only contiguous memory (the Raspberry Pi 5) can
  put them on a plane, else from gbm on a render node.
  `IHS_WL_STAGING_HEAP` names a heap under `/dev/dma_heap`;
  `IHS_WL_STAGING_DEVICE` names a render node and forces gbm. A single-pixel
  buffer goes the same way, as a 1x1 buffer of its color for the shell to
  stretch.
- Commits are held until their dma-bufs are ready, so the shell never needs
  an acquire fence. With explicit sync the commit's acquire point decides;
  otherwise the buffer's implicit fence does. A buffer's release point is
  signaled when it is released.
- Buffers are released to the client when the shell's release fence signals.
  Without a fence, a buffer is released once a later frame is on screen.
  Frame callbacks fire when a frame is shown.
- `zwp_linux_dmabuf_v1` v4 with default feedback naming the shell's render
  device as main device, so Mesa's Wayland EGL and Vulkan, GTK and Qt render
  on the GPU the shell imports on. A shell that cannot tell its device gets
  v3.
- Per-surface feedback with a scanout tranche. When the shell reports that a
  surface's layer could go on a KMS plane but for its buffer's format, that
  surface's feedback gets a scanout tranche for the KMS device, ahead of the
  main one: the planes' formats that the shell also imports. The client
  allocates its next buffers for the plane, and the layer bypasses
  composition. The tranche stays while the view shows the surface.
- Frame timing follows the shell's reports of shown frames. Presentation
  feedback carries the display's time, refresh, counter and flags (including
  zero-copy). A fifo barrier clears once the update that set it is shown. A
  commit with a target time is applied one refresh ahead of the first vblank
  at or after it. For views off screen, a clock at the display's refresh
  stands in for the reports.
- Popups (menus, tooltips) show above their toplevel in its view, placed
  inside it as their positioner allows. A popup that grabs keeps input on
  its client until a press lands outside it; popups also close when their
  view loses keyboard focus or leaves the scene.
- Scale follows the view's device pixel ratio (the widget passes it at
  creation). The toplevel is configured in logical pixels, and every surface
  is told the ratio through `wp_fractional_scale_v1`, rounded up through
  `wl_surface.preferred_buffer_scale`, and through the output it enters. A
  client that renders at it attaches buffers the size of the view on screen,
  which the shell shows 1:1.
- Pointer, touch and keyboard input go straight from the Dart view to the
  module over FFI, in view-local logical pixels. What is under a point is
  hit-tested against the tree the view shows, input regions included. The
  view takes keyboard focus when pressed; keys are sent as evdev codes and
  clients repeat them from `wl_keyboard.repeat_info` (600 ms, 25 Hz). The
  keymap is xkbcommon's default, set by the `XKB_DEFAULT_*` variables, as
  the shell's own is.
- The mouse cursor over a view is the one its client asks for. The module
  draws none: `ihs_wl_pointer` returns the client's `wp_cursor_shape_v1`
  shape, and the view shows it as the matching Flutter cursor. A client that
  sets a cursor surface of its own gets the default arrow; one that hides the
  cursor (`IHS_WL_CURSOR_HIDDEN`) hides it.

With the `egl-wl-display` cargo feature (off by default), the server runs on
libwayland-server and binds an EGL display to it (`EGL_WL_bind_wayland_display`),
for EGL implementations whose Wayland clients share buffers over a protocol of
their own rather than linux-dmabuf. The EGL library serves that protocol. On a
shell that samples image layers (an EGL backend, platform-view ABI 1.16) the
display bound is the shell's own, and each buffer goes to it as an EGLImage, so
nothing about the buffer is lost, compression included. Elsewhere each buffer is
turned into a dma-buf through libgbm, which needs one that can describe such
buffers (one exporting `gbm_perform`, as
[CodeLinaro libgbm](https://git.codelinaro.org/clo/le/display/libgbm) does); some
implementations allocate compressed buffers by default, which can show corrupted
that way, so have the client's EGL share them uncompressed there (the server
logs a warning when it sees one). The feature links libwayland-server and loads
libEGL at run time; there is still no EGL context. An app turns the
feature on in its pubspec:

```yaml
hooks:
  user_defines:
    ihs_wayland_server:
      cargo_features: [egl-wl-display]
```

## Layout

| Path                        | What                                                        |
|-----------------------------|-------------------------------------------------------------|
| `src/`                      | The server, and the C ABI in `src/lib.rs`                   |
| `include/ihs_wl.h`          | The C header, generated by cbindgen                         |
| `dart/ihs_wayland_server/`  | Dart package: `WaylandServer` and `WaylandToplevelView`     |
| `examples/standalone.rs`    | Runs the server without a shell, for trying real clients    |
| `tests/`                    | Integration tests against a mock shell and a Wayland client |

## Building

This crate needs:

- **Rust 1.94.1 or newer.** `rust-toolchain.toml` pins 1.94.1, the oldest
  compiler a supported Yocto release ships, so nothing newer creeps in.
- **ivi-homescreen's shared library (`libihs_shared`), at platform-view ABI
  1.16 or newer.** The build finds it through `ivi-homescreen-shared.pc`,
  checks the ABI in its headers, and generates bindings from them.
- **clang**, for bindgen.

To install the shared library to a local prefix and build against it:

```sh
cmake -S <ivi-homescreen>/shared -B build-shared -G Ninja \
  -DCMAKE_INSTALL_PREFIX=$PWD/prefix -DENABLE_DLT=OFF
ninja -C build-shared install

export PKG_CONFIG_PATH=$PWD/prefix/lib64/pkgconfig   # or lib/pkgconfig
cargo build --release
```

The output is `target/release/libihs_wl_server.so`.

For a native build against a prefix outside `/usr`, the build embeds an rpath,
so `cargo test` runs without `LD_LIBRARY_PATH`. Cross builds and sysroot
builds never get an rpath. To cross-compile, set `PKG_CONFIG_SYSROOT_DIR` and
`PKG_CONFIG_PATH` as for any pkg-config consumer. In Yocto, the cargo bbclass
sets them for you.

### Yocto

meta-flutter's `ihs-wl-server` recipe builds and installs the library;
`PACKAGECONFIG` `egl-wl-display` turns on the cargo feature. The Rust each
release needs:

| Release | Rust | Layers |
|---|---|---|
| master | 1.98.1 | oe-core |
| wrynose | 1.94.1 | oe-core |
| scarthgap | 1.98.1 | meta-lts-mixins `scarthgap/rust` and meta-clang; the recipe is a dynamic layer on the former |

An app built in the image takes the installed library instead of building
its own; name it in the app's pubspec:

```yaml
hooks:
  user_defines:
    ihs_wayland_server:
      system_library: true
```

## Using it from Flutter

Add the Dart package, start the server once, and place a view where the
client should appear:

```dart
import 'package:ihs_wayland_server/ihs_wayland_server.dart';

final server = WaylandServer.start(); // loads the bundled libihs_wl_server.so
// Launch clients with WAYLAND_DISPLAY=${server.socketName}.

const WaylandToplevelView(appId: 'org.example.app');
```

A view shows the oldest toplevel with a matching app_id that no other view is
already showing, or with no app_id given, the oldest one at all. It stays empty
until such a client maps.

To show the very client you start, whatever else runs with the same app_id,
launch it with an activation token and name the token in its view:

```dart
final (:process, :token) = await server.launch('gnome-calculator', []);

WaylandToplevelView(activationToken: token);
```

`launch` sets `WAYLAND_DISPLAY` and `XDG_ACTIVATION_TOKEN` for the client;
`server.activationToken()` gives a token for a launcher of your own. The client
activates its toplevel with the token (GTK and Qt do this unprompted), and the
view binds that toplevel. Views binding by app_id pass over toplevels launched
with a token. A client that ignores `XDG_ACTIVATION_TOKEN` is never found by
one; show it by app_id instead.

The package's build hook builds the crate with cargo as part of the app's
build and bundles the library, so there is nothing to build by hand. The
hook runs with a filtered environment: a cross build puts a `cargo` wrapper
carrying the target, linker and pkg-config settings first on `PATH` (emb does
this), and `ivi-homescreen-shared.pc` outside the default search path is named
in the app's pubspec:

```yaml
hooks:
  user_defines:
    ihs_wayland_server:
      pkg_config_path: /path/to/prefix/lib/pkgconfig
```

`dart/ihs_wayland_server/example` is a minimal app; its `emb.yaml` builds it
with emb against ivi-homescreen.

`WaylandServer.start` is idempotent. After a hot restart the server is still
running. Its clients carry on, and the re-created views bind to their
toplevels again.

By default the socket is named `wayland-ihs-N` when the shell is itself a
Wayland client (the wayland backends), and `wayland-N` otherwise.

## Trying it without a shell

```sh
cargo run --example standalone -- wayland-test 60
WAYLAND_DISPLAY=wayland-test foot
```

Clients connect and map, but nothing is shown, because there is no shell to
host the views.

## Explicit sync

Explicit sync is offered when a render node can wait on timeline syncobjs
through eventfds (Linux 6.6 or newer). A syncobj file is not tied to one
device, so the server uses the first `/dev/dri/renderD*` that can, whichever
GPU clients render on. `IHS_WL_SYNCOBJ_DEVICE` names a render node to use
instead. Without one, the global is not advertised and clients use implicit
sync.

## Logging

Logging uses `tracing`. The filter comes from `IHS_WL_LOG`, in `EnvFilter`
syntax; the default is `info,smithay=warn`.

## Testing

```sh
cargo test
cargo clippy --all-targets
cargo fmt --check
```

The integration tests run the server against a mock of the shell's
platform-view interface and drive it with a real Wayland client. In place of
dma-bufs, the client uses memfds; the server never reads buffer contents.

## License

Apache-2.0; see [LICENSE](LICENSE).
