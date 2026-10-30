// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

import 'dart:async';
import 'dart:ffi' as ffi;
import 'dart:isolate';

import 'bindings.g.dart';

/// View events the server posts to this isolate's port, handed to the view
/// with that id. Each message is `[event, viewId]`, `event` an
/// [IhsWlViewEvent] value, or `[event, toplevelId]` for [toplevels].
///
/// A view registers its id before the platform view is created, so a bind
/// that lands while the create is in flight is not lost. Events for ids no
/// view registered (one already disposed) are dropped.
final class ViewEvents {
  ViewEvents._();

  static final Map<int, void Function(IhsWlViewEvent event)> _views =
      <int, void Function(IhsWlViewEvent event)>{};

  static ReceivePort? _port;

  /// Window events, as `(event, toplevelId)`.
  static final StreamController<(IhsWlToplevelEvent, int)> toplevels =
      StreamController<(IhsWlToplevelEvent, int)>.broadcast();

  /// Receive events from @p lib's server on this isolate. After a hot
  /// restart the new isolate registers its own port; the old one is gone,
  /// and posting to it only fails.
  static void listen(IhsWlBindings lib) {
    final ReceivePort port = _port ??= ReceivePort('ihs_wl view events')
      ..listen(dispatch);
    lib.ihs_wl_set_event_port(
      ffi.NativeApi.postCObject.cast(),
      port.sendPort.nativePort,
    );
  }

  /// Stop receiving; the server no longer posts.
  static void close(IhsWlBindings lib) {
    lib.ihs_wl_set_event_port(ffi.nullptr, 0);
    _port?.close();
    _port = null;
  }

  static void watch(int viewId, void Function(IhsWlViewEvent event) onEvent) =>
      _views[viewId] = onEvent;

  static void unwatch(int viewId) => _views.remove(viewId);

  /// Hand @p message, as the server posts it, to its view.
  static void dispatch(Object? message) {
    if (message is! List<Object?> || message.length < 2) {
      return;
    }
    final Object? event = message[0];
    final Object? id = message[1];
    if (event is! int || id is! int) {
      return;
    }
    if (event >= IhsWlToplevelEvent.IHS_WL_TOPLEVEL_EVENT_MAPPED.value) {
      _toplevel(event, id);
      return;
    }
    final IhsWlViewEvent kind;
    try {
      kind = IhsWlViewEvent.fromValue(event);
    } on ArgumentError {
      // A newer server's event this package does not know.
      return;
    }
    _views[id]?.call(kind);
  }

  static void _toplevel(int event, int toplevelId) {
    final IhsWlToplevelEvent kind;
    try {
      kind = IhsWlToplevelEvent.fromValue(event);
    } on ArgumentError {
      // A newer server's event this package does not know.
      return;
    }
    toplevels.add((kind, toplevelId));
  }
}
