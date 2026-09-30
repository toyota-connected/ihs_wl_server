// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

part of 'view.dart';

/// A window-state change a client asked for: its maximize, restore,
/// minimize or fullscreen buttons, a title bar double-click, F11. The
/// toplevel stays the size of its view whatever it asks; the app decides
/// what, if anything, the view does.
enum WaylandWindowRequest {
  maximize,
  unmaximize,
  minimize,
  fullscreen,
  unfullscreen,
}

/// Controls the client toplevel a `WaylandToplevelView` shows, and reports
/// what happens to it.
///
/// Notifies its listeners when [isBound] changes: true once a toplevel is
/// shown, false again when the client closes it (after [close], its own
/// close button, or quitting). The view then shows nothing until another
/// matching toplevel maps.
class WaylandToplevelController extends ChangeNotifier {
  WaylandToplevelController({this.onWindowRequest});

  /// Called when the client asks to change its window state.
  void Function(WaylandWindowRequest request)? onWindowRequest;

  int? _viewId;
  bool _bound = false;

  /// Whether the view shows a toplevel.
  bool get isBound => _bound;

  /// Ask the client to close its toplevel (`xdg_toplevel.close`). The client
  /// decides: it may ask its user first (unsaved changes, say), or refuse.
  /// When it closes, [isBound] turns false. Does nothing while nothing is
  /// shown.
  void close() {
    final int? id = _viewId;
    if (id != null && _bound) {
      waylandInput?.closeView(id);
    }
  }

  /// The view this controls, and whether it shows a toplevel; null when
  /// detached.
  void _attach(int? viewId, bool bound) {
    _viewId = viewId;
    _setBound(bound);
  }

  void _setBound(bool value) {
    if (_bound != value) {
      _bound = value;
      notifyListeners();
    }
  }
}
