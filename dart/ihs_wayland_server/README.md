# ihs_wayland_server

Run Wayland clients inside an
[ivi-homescreen](https://github.com/toyota-connected/ivi-homescreen) Flutter
app. The package starts an embedded Wayland server,
[ihs_wl_server](https://github.com/toyota-connected/ihs_wl_server), in the
shell's process and shows each client toplevel as a platform view, with
pointer, touch, keyboard and cursor input forwarded to the client.

Linux only.

## Requirements

- ivi-homescreen with platform-view ABI 1.16 or newer.
- `libihs_wl_server.so`, obtained one of two ways:
  - **Installed.** A Yocto image with meta-flutter's `ihs-wl-server` recipe,
    or a library built and installed from the ihs_wl_server repository. The
    package loads it by name, or from `IHS_WL_LIB`.
  - **Built by the package.** Depended on from the ihs_wl_server repository
    (a git or path dependency), the package's build hook builds the crate
    with cargo as part of the app's build and bundles the library. This needs
    Rust and ivi-homescreen's `libihs_shared`; see the repository's README.

From pub.dev nothing is built: the installed library is used.

An app built in a Yocto image names the installed library in its pubspec so
the hook never builds one:

```yaml
hooks:
  user_defines:
    ihs_wayland_server:
      system_library: true
```

## Usage

Start the server once, and place a view where the client should appear:

```dart
import 'package:ihs_wayland_server/ihs_wayland_server.dart';

final server = WaylandServer.start();
// Clients connect with WAYLAND_DISPLAY=${server.socketName}.

WaylandToplevelView(appId: 'weston-simple-egl');
```

Or launch the client and bind the view to it by activation token:

```dart
final (:process, :token) = await server.launch('gnome-calculator', []);

WaylandToplevelView(activationToken: token);
```

`launch` sets `WAYLAND_DISPLAY` and `XDG_ACTIVATION_TOKEN` for the client.
GTK and Qt clients activate their toplevel with the token unprompted; show a
client that ignores it by app_id instead. Without either, a view shows the
oldest toplevel not shown elsewhere.

`WaylandServer.start` is idempotent: after a hot restart the server and its
clients carry on, and the re-created views bind to their toplevels again.

See `example/` for a minimal app, and the
[repository](https://github.com/toyota-connected/ihs_wl_server) for the
protocols supported, buffer paths and build options.
