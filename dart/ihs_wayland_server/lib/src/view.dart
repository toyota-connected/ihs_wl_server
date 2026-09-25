// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'server.dart';

/// The platform-view type the module's factory registers.
const String _viewType = 'ihs_wl/toplevel';

/// Shows one Wayland client toplevel.
///
/// The toplevel is found by [activationToken] (the token the launcher put in
/// the client's `XDG_ACTIVATION_TOKEN`), or else by [appId]: the oldest
/// toplevel with that app_id not already shown elsewhere. With neither, it
/// is the oldest toplevel not shown elsewhere, whatever its app_id. Until a
/// matching client maps, the view is empty.
///
/// Pointer and touch input over the view go to the client under it. The
/// view takes keyboard focus when pressed and gives it up on a press
/// anywhere else; while it has focus every key goes to the client.
class WaylandToplevelView extends StatefulWidget {
  const WaylandToplevelView({
    super.key,
    this.activationToken,
    this.appId,
    this.focusNode,
    this.autofocus = false,
  });

  final String? activationToken;
  final String? appId;

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

  @override
  void dispose() {
    _ownFocus?.dispose();
    super.dispose();
  }

  void _created(int id) {
    _viewId = id;
    if (_focus.hasFocus) {
      waylandInput?.focus(id, true);
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
            child: PlatformViewLink(
              viewType: _viewType,
              surfaceFactory:
                  (BuildContext context, PlatformViewController controller) {
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
                // PlatformViewLink calls create(size:) once laid out, so the
                // view is created at its real size rather than 0x0.
                return _ToplevelViewController(
                  id: params.id,
                  creationParams: <String, Object?>{
                    'token': widget.activationToken,
                    'app_id': widget.appId,
                    'dpr': dpr,
                  },
                  onCreated: (int id) {
                    _created(id);
                    params.onPlatformViewCreated(id);
                  },
                );
              },
            ),
          ),
        ),
      ),
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
