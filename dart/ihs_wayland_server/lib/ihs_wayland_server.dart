// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

/// Dart side of ihs_wl_server: start the embedded Wayland server and show a
/// client toplevel as an ivi-homescreen platform view.
library;

export 'src/server.dart' show WaylandServer, WaylandServerException;
export 'src/view.dart' show WaylandToplevelView;
