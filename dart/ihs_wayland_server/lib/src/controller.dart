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

/// A window-state change the app handles for a view. Its client shows the
/// buttons for these and no others, and its [WaylandWindowRequest]s for
/// others are not reported.
enum WaylandWindowCapability {
  /// Maximize and restore.
  maximize(1),
  minimize(2),

  /// Fullscreen and back.
  fullscreen(4);

  const WaylandWindowCapability(this._bit);

  final int _bit;
}

/// A window state the app grants a client: it is told it is maximized or
/// fullscreen, and draws itself so (a fullscreen GTK window hides its header
/// bar). Its size stays the view's; the app sizes the view.
enum WaylandWindowState {
  maximized(1),
  fullscreen(2);

  const WaylandWindowState(this._bit);

  final int _bit;
}

/// Controls the client toplevel a `WaylandToplevelView` shows, and reports
/// what happens to it.
///
/// Notifies its listeners when [isBound] changes: true once a toplevel is
/// shown, false again when the client closes it (after [close], its own
/// close button, or quitting). The view then shows nothing until another
/// matching toplevel maps.
class WaylandToplevelController extends ChangeNotifier {
  WaylandToplevelController({
    this.onWindowRequest,
    Set<WaylandWindowCapability> windowCapabilities =
        const <WaylandWindowCapability>{},
  }) : _capabilities = Set<WaylandWindowCapability>.unmodifiable(
         windowCapabilities,
       );

  Set<WaylandWindowState> _state = const <WaylandWindowState>{};

  /// The window states the app grants the client, usually in answer to its
  /// [onWindowRequest]s: grant [WaylandWindowState.fullscreen] on a
  /// fullscreen request, clear it on unfullscreen. Neither by default. A
  /// request not answered leaves the client as it was, which some clients
  /// (a GTK F11 toggle) take as a refusal.
  Set<WaylandWindowState> get windowState => _state;
  set windowState(Set<WaylandWindowState> value) {
    if (setEquals(value, _state)) {
      return;
    }
    _state = Set<WaylandWindowState>.unmodifiable(value);
    final int? id = _viewId;
    if (id != null) {
      waylandInput?.setWindowState(id, _stateBits);
    }
  }

  /// [windowState] as `IhsWlWindowState` bits.
  int get _stateBits =>
      _state.fold(0, (int bits, WaylandWindowState s) => bits | s._bit);

  /// Called when the client asks to change its window state, for the
  /// [windowCapabilities] the app handles.
  void Function(WaylandWindowRequest request)? onWindowRequest;

  Set<WaylandWindowCapability> _capabilities;

  /// The window-state changes the app handles: the client shows buttons for
  /// these and no others. None by default.
  Set<WaylandWindowCapability> get windowCapabilities => _capabilities;
  set windowCapabilities(Set<WaylandWindowCapability> value) {
    if (setEquals(value, _capabilities)) {
      return;
    }
    _capabilities = Set<WaylandWindowCapability>.unmodifiable(value);
    final int? id = _viewId;
    if (id != null) {
      waylandInput?.setCapabilities(id, _capabilityBits);
    }
  }

  /// [windowCapabilities] as `IhsWlCapability` bits.
  int get _capabilityBits => _capabilities.fold(
    0,
    (int bits, WaylandWindowCapability c) => bits | c._bit,
  );

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
