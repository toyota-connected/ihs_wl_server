/// Dart side of ihs_wl_server: start the embedded Wayland server and show a
/// client toplevel as an ivi-homescreen platform view.
library;

export 'src/server.dart' show WaylandServer, WaylandServerException;
export 'src/view.dart' show WaylandToplevelView;
