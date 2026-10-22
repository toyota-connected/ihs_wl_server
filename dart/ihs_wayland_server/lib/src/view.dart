// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'bindings.g.dart' show IhsWlViewEvent;
import 'client.dart';
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
/// A client sandboxed by an engine such as flatpak (wp_security_context_v1)
/// matches [appId] only when it is its sandbox's app id. With a
/// [sandboxAppId], only a toplevel of a client in that sandbox matches,
/// whichever way it is found and whatever app id the client sets.
///
/// The client is asked for the view's size as it is laid out, and follows
/// it through every layout change. With a [requestedSize] it is asked for
/// that size instead, and its content is scaled to fit the view, aspect
/// kept, centered.
///
/// A view an ancestor draws scaled (a [FittedBox], a [Transform.scale]) is
/// sized, and takes input, as it is on screen: the shell draws the client's
/// buffers into the view's rect on screen, so the client renders at 1:1
/// there rather than at the size of the app's layout.
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
    this.sandboxAppId,
    this.requestedSize,
    this.controller,
    this.focusNode,
    this.autofocus = false,
  });

  /// The token the client was launched with.
  final String? activationToken;
  final String? appId;

  /// The sandbox app id the client must run under, e.g. a flatpak app id.
  final String? sandboxAppId;

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

  /// The device pixel ratio the view was last built at.
  double _dpr = 1.0;

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
    // As the creation params carried them; the controller may have changed.
    _sendController(id);
    // The creation params carry the size the view was built with; this one
    // may have changed since.
    waylandInput?.requestSize(id, widget.requestedSize);
    if (_focus.hasFocus) {
      waylandInput?.focus(id, true);
    }
  }

  /// What the controller sets for the view, or the defaults without one.
  void _sendController(int id) {
    final WaylandToplevelController? controller = widget.controller;
    waylandInput?.setCapabilities(id, controller?._capabilityBits ?? 0);
    waylandInput?.setWindowState(id, controller?._stateBits ?? 0);
  }

  @override
  void didUpdateWidget(WaylandToplevelView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.controller != oldWidget.controller) {
      oldWidget.controller?._attach(null, false);
      widget.controller?._attach(_viewId, _bound);
      final int? id = _viewId;
      if (id != null) {
        _sendController(id);
      }
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
    _dpr = dpr;
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
                        // The shell learns a new size only from a resize
                        // call; PlatformViewLink creates at the first size
                        // and never reports another.
                        final _ToplevelViewController view =
                            controller as _ToplevelViewController;
                        return _LayoutListener(
                          onLayout: view.laidOut,
                          onPaint: (double scale) => view.drawnAt(scale / _dpr),
                          child: PlatformViewSurface(
                            controller: controller,
                            gestureRecognizers:
                                const <Factory<OneSequenceGestureRecognizer>>{
                                  Factory<OneSequenceGestureRecognizer>(
                                    EagerGestureRecognizer.new,
                                  ),
                                },
                            hitTestBehavior: PlatformViewHitTestBehavior.opaque,
                          ),
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
                        'sandbox_app_id': widget.sandboxAppId,
                        'dpr': dpr,
                        'capabilities': widget.controller?._capabilityBits ?? 0,
                        'window_state': widget.controller?._stateBits ?? 0,
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

  /// The size the shell was last given, at create or resize: the view's on
  /// screen.
  Size? _sentSize;

  /// The size the view was last laid out at.
  Size? _laidOut;

  /// Logical pixels on screen per logical pixel of the view's layout: not 1
  /// where an ancestor draws it scaled.
  double _scale = 1.0;

  /// @p size, laid out, as it is on screen.
  Size _onScreen(Size size) => size * _scale;

  /// The view was laid out at @p size: tell the shell if it has another.
  void laidOut(Size size) {
    _laidOut = size;
    _sendSize();
  }

  /// The view was drawn @p scale times its layout size on screen. Called
  /// from paint; the calls it makes change no widget state.
  void drawnAt(double scale) {
    if ((scale - _scale).abs() < 1e-3) {
      return;
    }
    _scale = scale;
    // The shell draws the client's buffers into the view's rect on screen,
    // and the server places them in the size it was given: give it that
    // rect's, and input to match.
    waylandInput?.setViewScale(id, scale);
    _sendSize();
  }

  void _sendSize() {
    final Size? laidOut = _laidOut;
    if (_created && laidOut != null && _onScreen(laidOut) != _sentSize) {
      _resize(_onScreen(laidOut));
    }
  }

  void _resize(Size size) {
    _sentSize = size;
    SystemChannels.platform_views.invokeMethod<void>(
      'resize',
      <String, Object?>{'id': id, 'width': size.width, 'height': size.height},
    );
  }

  Future<void> _createOnce(Size? laidOut) async {
    final Size? size = laidOut == null ? null : _onScreen(laidOut);
    _sentSize = size;
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
    if (_scale != 1.0) {
      waylandInput?.setViewScale(id, _scale);
    }
    onCreated(id);
    // Laid out again, or drawn at another scale, while the create was in
    // flight.
    _sendSize();
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
    waylandInput?.setViewScale(id, 1.0);
    await SystemChannels.platform_views.invokeMethod<void>(
      'dispose',
      <String, Object>{'id': id},
    );
  }
}

/// Reports its size after every layout that changes it, and the scale it is
/// drawn at on screen after every paint.
class _LayoutListener extends SingleChildRenderObjectWidget {
  const _LayoutListener({
    required this.onLayout,
    required this.onPaint,
    required super.child,
  });

  final ValueChanged<Size> onLayout;

  /// Physical pixels on screen per logical pixel of its layout, as painted.
  final ValueChanged<double> onPaint;

  @override
  _RenderLayoutListener createRenderObject(BuildContext context) =>
      _RenderLayoutListener(onLayout, onPaint);

  @override
  void updateRenderObject(
    BuildContext context,
    _RenderLayoutListener renderObject,
  ) {
    renderObject
      ..onLayout = onLayout
      ..onPaint = onPaint;
  }
}

class _RenderLayoutListener extends RenderProxyBox {
  _RenderLayoutListener(this.onLayout, this.onPaint);

  ValueChanged<Size> onLayout;
  ValueChanged<double> onPaint;
  Size? _last;

  @override
  void performLayout() {
    super.performLayout();
    if (size != _last) {
      _last = size;
      onLayout(size);
    }
  }

  @override
  void paint(PaintingContext context, Offset offset) {
    super.paint(context, offset);
    // Only final at paint: a FittedBox sets its scale after laying out its
    // child. To the screen, in physical pixels: the root applies the device
    // pixel ratio, and the ancestors (a FittedBox, a Transform.scale) the
    // rest.
    final Matrix4 m = getTransformTo(null);
    // The length of the x unit vector: the scale, rotation aside.
    final double x = m.entry(0, 0);
    final double y = m.entry(1, 0);
    onPaint(math.sqrt(x * x + y * y));
  }
}
