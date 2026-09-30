<!--
SPDX-FileCopyrightText: 2026 Toyota Connected North America
SPDX-License-Identifier: Apache-2.0
-->

# Development

Building and testing the crate and the Dart package on a host, then running
the example app with real Wayland clients on a Raspberry Pi 5. The README
covers the crate build itself ([Building](../README.md#building)) and the
unit/integration tests ([Testing](../README.md#testing)).

## Host

Tools:

- Rust through rustup: `rust-toolchain.toml` pins the toolchain, and rustup
  installs it on first use.
- clang (bindgen), cmake, ninja, pkg-config.
- `libihs_shared` installed to a prefix, from an ivi-homescreen checkout at
  platform-view ABI 1.16 or newer (see [Building](../README.md#building)).
- A Flutter SDK, for the Dart package and the example.
- [emb](https://github.com/meta-flutter/workspace-automation), for cross
  builds to the Pi.
- cbindgen 0.27 or newer and ffigen (a dev dependency of the package), only
  to regenerate the C header and Dart bindings.

Checks:

```sh
export PKG_CONFIG_PATH=<prefix>/lib64/pkgconfig   # or lib/pkgconfig
cargo test
cargo clippy --all-targets
cargo clippy --all-targets --features egl-wl-display
cargo fmt --check
```

The Dart package's tests load the library its build hook builds, so the hook
needs `ivi-homescreen-shared.pc` too. Hooks run with a filtered environment
that drops `PKG_CONFIG_PATH`, so name the prefix in the package's pubspec for
the run (do not commit it):

```yaml
hooks:
  user_defines:
    ihs_wayland_server:
      pkg_config_path: <prefix>/lib64/pkgconfig
```

```sh
cd dart/ihs_wayland_server
flutter analyze
flutter test
dart format --output=none --set-exit-if-changed lib hook test example/lib
```

After a change to the C ABI in `src/lib.rs`:

```sh
cbindgen --config cbindgen.toml --output include/ihs_wl.h
cd dart/ihs_wayland_server
dart run ffigen --config ffigen.yaml \
  --compiler-opts "-I$(clang -print-resource-dir)/include"
dart format lib/src/bindings.g.dart
```

## Raspberry Pi 5

Raspberry Pi OS (Debian trixie), 64-bit. The shell takes DRM master on the
HDMI output, so nothing else may hold it: stop a desktop session, or run from
a console or over ssh with no compositor running.

### Packages

Test clients:

```sh
sudo apt install gnome-calculator gtk-4-examples foot chromium mpv ffmpeg
```

| Package | Clients | Exercises |
|---|---|---|
| `gnome-calculator` | `gnome-calculator` | launching by activation token (GTK 4, libadwaita) |
| `gtk-4-examples` | `gtk4-widget-factory`, `gtk4-demo` | popups (menus), dialogs (About, Print), window buttons, F11, a minimum size wider than the view |
| `foot` | `foot` | a simple client that honors its configured size; keyboard |
| `chromium` | `chromium` | a large client; video and WebGL |
| `mpv` | `mpv` | video (dma-buf, YUV) |
| `ffmpeg` | `ffmpeg` | making test clips for mpv and chromium |

Optional:

```sh
sudo apt install weston cage wayland-utils vulkan-tools
```

`weston` (a reference compositor, and the `weston-simple-*` clients),
`cage` (to compare a client under another compositor), `wayland-info`,
`vkcube`.

The runtime libraries the module needs (libgbm, libxkbcommon, libdrm) are part
of a standard image.

### Building the example

From an ivi-homescreen checkout, with emb on `PATH`:

```sh
emb cross .emb/raspberry-pi.emb.yaml --target rpi5-trixie --build \
  --backend drm-kms-egl -D BUILD_COMPOSITOR=ON \
  --app <ihs_wl_server>/dart/ihs_wayland_server/example
```

`--backend drm-kms-vulkan` builds the Vulkan backend instead. The example's
build hook cross-compiles `libihs_wl_server.so` with cargo, and emb collects
the shell, the engine and the app into a `runnable/` directory under its
workspace. Copy it to the Pi:

```sh
rsync -a --delete <runnable>/ <pi>:ihs-wl-example/
```

### Running

On the Pi, in `~/ihs-wl-example`:

```sh
XDG_RUNTIME_DIR=/run/user/$(id -u) \
DBUS_SESSION_BUS_ADDRESS=disabled: \
LAUNCH=gtk4-widget-factory \
./homescreen -b . --backend drm-kms-egl \
  --drm-device /dev/dri/card0 --drm-connector HDMI-A-1 -f
```

- `LAUNCH=<client>`: the example launches that client itself with an
  activation token and shows its toplevel. Try `gnome-calculator`,
  `gtk4-widget-factory`, `foot`.
- Without `LAUNCH`, the example shows the first toplevel to map. Its bar shows
  the socket (`WAYLAND_DISPLAY=wayland-0` on the DRM backends); from another
  shell: `WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/$(id -u) foot`.
  Building with `--dart-define=APP_ID=<app_id>` makes it show that app only.
- `--backend` must match the backend the runnable was built for.
- Stop it with `pkill -f homescreen`; clients the example launched exit with
  it.

The example's bar has Scale 800x600 (the client asked for a fixed size and
scaled to fit) and Close. It grants maximize and fullscreen, which hide the
bar, and its minimize hides the view behind Restore (the client is told
`suspended`).

### Vulkan backend

Build with `--backend drm-kms-vulkan`. A runnable holds one backend, so keep
one directory per backend (`~/ihs-wl-example-vk`, say). Run it with the same
environment as above and:

```sh
./homescreen -b . --backend drm-kms-vulkan \
  --drm-device /dev/dri/card0 --drm-connector HDMI-A-1 -f
```

Options:

- `IVI_DRMVK_PLANE_LAYERS=1`: give each layer its own KMS plane, so a client's
  buffers are scanned out directly. By default the backend blends all layers
  into one with Vulkan.
- `--drm-explicit-sync auto|yes|no`: fence the commits with `IN_FENCE_FD`
  (`no` takes the CPU-fence path; `IVI_DRMVK_NO_EXPLICIT_SYNC` is the older
  spelling of `no`).
- `IVI_DRMVK_VSYNC=0`: pace on the wall clock instead of page-flip events, to
  bisect pacing problems.

On the Pi 5 (Mesa 25.0, v3dv), small client buffers (GTK menus) only import
with
[ivi-homescreen#694](https://github.com/toyota-connected/ivi-homescreen/pull/694).

Vulkan clients:

- `vkcube --wsi wayland` (`vulkan-tools`).
- `mpv --gpu-api=vulkan` fails on v3dv (its swapchain runs out of memory); use
  `--gpu-api=opengl`.

### Client notes

- **GTK 4** (and Chromium): with no `WAYLAND_DISPLAY` in the Pi's session,
  xdg-desktop-portal cannot start, and GTK hangs at startup reading its
  settings from it. `DBUS_SESSION_BUS_ADDRESS=disabled:` skips the portal.
- **GTK window buttons**: GTK reads its button layout from GSettings
  (`org.gnome.desktop.wm.preferences button-layout`), `appmenu:close` by
  default here, so it shows no minimize or maximize even when the example
  advertises them. For one run, without touching your settings:

  ```sh
  mkdir -p /tmp/gtk-cfg/glib-2.0/settings
  printf "[org/gnome/desktop/wm/preferences]\nbutton-layout='appmenu:minimize,maximize,close'\n" \
    > /tmp/gtk-cfg/glib-2.0/settings/keyfile
  GSETTINGS_BACKEND=keyfile XDG_CONFIG_HOME=/tmp/gtk-cfg ...
  ```

- **gtk4-widget-factory** has a minimum width (about 1337, with the default
  fonts) wider than a 1280 view; the server scales it down to fit rather than
  cut its right edge off.
- **Chromium**: `--ozone-platform=wayland --password-store=basic`. No HEVC
  `<video>`.
- **mpv**: `--gpu-api=opengl`; its Vulkan output fails on v3dv, and
  `--vo=dmabuf-wayland` crashes on a sized configure before the first frame
  (an mpv bug, also under cage).

### Debugging

- `IHS_WL_LOG` filters the module's log (`EnvFilter`), default
  `info,smithay=warn`. Useful targets: `ihs_wl_server::handlers=debug` (window
  requests, toplevels), `ihs_wl_server::view=debug` (resize, suspension),
  `ihs_wl_server::scanout=debug`, `ihs_wl_server::pacing=debug`;
  `ihs_wl_server::observe=debug` logs every submit.
- `WAYLAND_DEBUG=client` in the shell's environment reaches a client the
  example launches, which logs its whole protocol traffic (configures, buffer
  attaches, frame callbacks) to the shell's output. The DRM backends are not
  Wayland clients, so the shell itself is unaffected.

Shell issues found this way:

- drm-kms-egl stops presenting after a client maximizes
  ([ivi-homescreen#690](https://github.com/toyota-connected/ivi-homescreen/issues/690)).
- drm-kms-vulkan could not import small, exactly sized dma-bufs on v3dv, so
  GTK menus never appeared
  ([#691](https://github.com/toyota-connected/ivi-homescreen/issues/691),
  worked around in
  [#694](https://github.com/toyota-connected/ivi-homescreen/pull/694)).
- A view that leaves Flutter's scene (Offstage, scrolled off) is suspended
  only with
  [#693](https://github.com/toyota-connected/ivi-homescreen/pull/693).
