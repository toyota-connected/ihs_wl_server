// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'bindings.g.dart' show IhsWlViewEvent;
import 'events.dart';
import 'input.dart';
import 'server.dart';

part 'controller.dart';

/// The platform-view type the module's factory registers.
const String _viewType = 'ihs_wl/toplevel';

/// Shows one Wayland client toplevel.
///
/// With an [activationToken] (from [WaylandServer.activationToken] or
/// [WaylandServer.launch]), the toplevel is the one its client activates
/// with that token, as GTK and Qt clients started with it in
/// `XDG_ACTIVATION_TOKEN` do; [appId] is not used then. Without one, it is
/// the oldest toplevel with [appId] not already shown elsewhere, or with
/// neither, the oldest toplevel not shown elsewhere -- in both cases passing
/// over toplevels launched with a token, which are for the views naming
/// theirs. Until a matching client maps, the view is empty.
///
/// The client is asked for the view's size as it is laid out, and follows
/// it through every layout change. With a [requestedSize] it is asked for
/// that size instead, and its content is scaled to fit the view, aspect
/// kept, centered.
///
/// A [controller] reports whether a toplevel is shown and the window-state
/// changes its client asks for, and asks the client to close.
///
/// Pointer and touch input over the view go to the client under it, and the
/// mouse cursor over it is the one the client asks for. The view takes
/// keyboard focus when pressed and gives it up on a press anywhere else;
/// while it has focus every key goes to the client.
class WaylandToplevelView extends StatefulWidget {
  const WaylandToplevelView({
    super.key,
    this.activationToken,
    this.appId,
    this.requestedSize,
    this.controller,
    this.focusNode,
    this.autofocus = false,
  });

  /// The token the client was launched with.
  final String? activationToken;
  final String? appId;

  /// The size to ask the client for, in logical pixels, instead of the
  /// view's. Its content is scaled to fit the view, aspect kept, centered,
  /// and input is mapped back. A client may still commit another size; it
  /// is then scaled as if it had taken this one. Null follows the view.
  final Size? requestedSize;

  /// Reports what happens to the toplevel shown, and closes it.
  final WaylandToplevelController? controller;

  /// Keyboard focus for the client; one is made when null.
  final FocusNode? focusNode;
  final bool autofocus;

  @override
  State<WaylandToplevelView> createState() => _WaylandToplevelViewState();
}

class _WaylandToplevelViewState extends State<WaylandToplevelView> {
  FocusNode? _ownFocus;
  FocusNode get _focus =>
      widget.focusNode ??
      (_ownFocus ??= FocusNode(debugLabel: 'WaylandToplevelView'));

  /// The platform-view id, once created.
  int? _viewId;

  /// The id events are taken for: known before the view is created.
  int? _watchedId;

  /// A toplevel is shown.
  bool _bound = false;

  @override
  void dispose() {
    final int? watched = _watchedId;
    if (watched != null) {
      ViewEvents.unwatch(watched);
    }
    widget.controller?._attach(null, false);
    _ownFocus?.dispose();
    super.dispose();
  }

  void _watch(int id) {
    final int? old = _watchedId;
    if (old != null) {
      ViewEvents.unwatch(old);
    }
    _watchedId = id;
    _bound = false;
    ViewEvents.watch(id, _onViewEvent);
  }

  void _onViewEvent(IhsWlViewEvent event) {
    final WaylandToplevelController? controller = widget.controller;
    final WaylandWindowRequest? request = switch (event) {
      IhsWlViewEvent.IHS_WL_VIEW_EVENT_BOUND ||
      IhsWlViewEvent.IHS_WL_VIEW_EVENT_CLOSED => null,
      IhsWlViewEvent.IHS_WL_VIEW_EVENT_MAXIMIZE_REQUESTED =>
        WaylandWindowRequest.maximize,
      IhsWlViewEvent.IHS_WL_VIEW_EVENT_UNMAXIMIZE_REQUESTED =>
        WaylandWindowRequest.unmaximize,
      IhsWlViewEvent.IHS_WL_VIEW_EVENT_MINIMIZE_REQUESTED =>
        WaylandWindowRequest.minimize,
      IhsWlViewEvent.IHS_WL_VIEW_EVENT_FULLSCREEN_REQUESTED =>
        WaylandWindowRequest.fullscreen,
      IhsWlViewEvent.IHS_WL_VIEW_EVENT_UNFULLSCREEN_REQUESTED =>
        WaylandWindowRequest.unfullscreen,
    };
    if (request != null) {
      controller?.onWindowRequest?.call(request);
      return;
    }
    _bound = event == IhsWlViewEvent.IHS_WL_VIEW_EVENT_BOUND;
    controller?._setBound(_bound);
  }

  void _created(int id) {
    _viewId = id;
    widget.controller?._attach(id, _bound);
    // The creation params carry the size the view was built with; this one
    // may have changed since.
    waylandInput?.requestSize(id, widget.requestedSize);
    if (_focus.hasFocus) {
      waylandInput?.focus(id, true);
    }
  }

  @override
  void didUpdateWidget(WaylandToplevelView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.controller != oldWidget.controller) {
      oldWidget.controller?._attach(null, false);
      widget.controller?._attach(_viewId, _bound);
    }
    final int? id = _viewId;
    if (id != null && widget.requestedSize != oldWidget.requestedSize) {
      waylandInput?.requestSize(id, widget.requestedSize);
    }
  }

  void _focusChanged(bool focused) {
    final int? id = _viewId;
    if (id != null) {
      waylandInput?.focus(id, focused);
    }
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    final int? id = _viewId;
    if (id == null || !(waylandInput?.key(id, event) ?? false)) {
      return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  void _onSignal(PointerSignalEvent event) {
    final int? id = _viewId;
    if (id == null || event is! PointerScrollEvent) {
      return;
    }
    GestureBinding.instance.pointerSignalResolver.register(event, (
      PointerSignalEvent event,
    ) {
      waylandInput?.scroll(id, event as PointerScrollEvent);
    });
  }

  void _onExit(PointerExitEvent event) {
    final int? id = _viewId;
    if (id != null) {
      waylandInput?.leave(id, event);
    }
  }

  @override
  Widget build(BuildContext context) {
    final Size? requested = widget.requestedSize;
    assert(
      requested == null || (requested.isFinite && !requested.isEmpty),
      'requestedSize must be finite and positive: $requested',
    );
    final double dpr = MediaQuery.devicePixelRatioOf(context);
    return Focus(
      focusNode: _focus,
      autofocus: widget.autofocus,
      onFocusChange: _focusChanged,
      onKeyEvent: _onKey,
      // A click elsewhere takes focus from the client, as a click off a
      // window does, even where nothing there takes focus itself.
      child: TapRegion(
        onTapOutside: (_) => _focus.unfocus(),
        child: MouseRegion(
          onExit: _onExit,
          child: Listener(
            // Takes focus even before the surface is there to hit.
            behavior: HitTestBehavior.opaque,
            onPointerDown: (_) => _focus.requestFocus(),
            onPointerSignal: _onSignal,
            child: Stack(
              children: <Widget>[
                PlatformViewLink(
                  viewType: _viewType,
                  surfaceFactory:
                      (
                        BuildContext context,
                        PlatformViewController controller,
                      ) {
                        return PlatformViewSurface(
                          controller: controller,
                          gestureRecognizers:
                              const <Factory<OneSequenceGestureRecognizer>>{
                                Factory<OneSequenceGestureRecognizer>(
                                  EagerGestureRecognizer.new,
                                ),
                              },
                          hitTestBehavior: PlatformViewHitTestBehavior.opaque,
                        );
                      },
                  onCreatePlatformView: (PlatformViewCreationParams params) {
                    // Before the create: a bind can land while it is in
                    // flight.
                    _watch(params.id);
                    // PlatformViewLink calls create(size:) once laid out, so
                    // the view is created at its real size rather than 0x0.
                    return _ToplevelViewController(
                      id: params.id,
                      creationParams: <String, Object?>{
                        'token': widget.activationToken,
                        'app_id': widget.appId,
                        'dpr': dpr,
                        'requested_width': widget.requestedSize?.width,
                        'requested_height': widget.requestedSize?.height,
                      },
                      onCreated: (int id) {
                        _created(id);
                        params.onPlatformViewCreated(id);
                      },
                    );
                  },
                ),
                // A platform view leaves the cursor to the platform: this
                // sets it, without taking the events from the view.
                Positioned.fill(child: _ClientCursor(input: waylandInput)),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// The cursor the client under the pointer asks for.
class _ClientCursor extends StatelessWidget {
  const _ClientCursor({required this.input});

  final WaylandInput? input;

  @override
  Widget build(BuildContext context) {
    final WaylandInput? input = this.input;
    if (input == null) {
      return const SizedBox.expand();
    }
    return ValueListenableBuilder<int>(
      valueListenable: input.cursor,
      builder: (BuildContext context, int shape, Widget? child) => MouseRegion(
        opaque: false,
        hitTestBehavior: HitTestBehavior.translucent,
        cursor: cursorFor(shape),
        child: child,
      ),
      child: const SizedBox.expand(),
    );
  }
}

class _ToplevelViewController extends PlatformViewController {
  _ToplevelViewController({
    required this.id,
    required this.creationParams,
    required this.onCreated,
  });

  final int id;
  final Map<String, Object?> creationParams;
  final PlatformViewCreatedCallback onCreated;

  bool _created = false;
  Future<void>? _creation;

  @override
  int get viewId => id;

  @override
  bool get awaitingCreation => !_created;

  @override
  Future<void> create({Size? size, Offset? position}) =>
      _creation ??= _createOnce(size);

  Future<void> _createOnce(Size? size) async {
    final ByteData? params = const StandardMessageCodec().encodeMessage(
      creationParams,
    );
    await SystemChannels.platform_views
        .invokeMethod<void>('create', <String, Object?>{
          'id': id,
          'viewType': _viewType,
          'direction': 0,
          'width': size?.width ?? 0.0,
          'height': size?.height ?? 0.0,
          if (params != null)
            'params': params.buffer.asUint8List(
              params.offsetInBytes,
              params.lengthInBytes,
            ),
        });
    _created = true;
    onCreated(id);
  }

  // Straight to the module over FFI: no platform channel, no platform
  // thread hop.
  @override
  Future<void> dispatchPointerEvent(PointerEvent event) async {
    if (_created) {
      waylandInput?.pointerEvent(id, event);
    }
  }

  @override
  Future<void> clearFocus() async {}

  @override
  Future<void> dispose() async {
    if (!_created) {
      return;
    }
    await SystemChannels.platform_views.invokeMethod<void>(
      'dispose',
      <String, Object>{'id': id},
    );
  }
}
