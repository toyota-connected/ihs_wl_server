// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

import 'dart:ffi' as ffi;

import 'package:ffi/ffi.dart';

import 'bindings.g.dart';
import 'loader.dart';

/// A failed call into libihs_wl_server.
class WaylandServerException implements Exception {
  WaylandServerException(this.call, this.code, this.message);

  final String call;

  /// The `IhsWlResult` value.
  final int code;
  final String message;

  @override
  String toString() => 'WaylandServerException: $call failed ($code): $message';
}

/// The process-wide embedded Wayland server.
///
/// Clients connect to [socketName]. Each toplevel is shown by a
/// `WaylandToplevelView`, which binds to it by activation token or app_id.
class WaylandServer {
  WaylandServer._(this._lib);

  final IhsWlBindings _lib;

  static WaylandServer? _instance;

  /// Load the module and start the server. Idempotent: after a hot restart
  /// the server is already running, and this returns a handle to it.
  ///
  /// [socketName] defaults to `wayland-ihs-N` when the shell is itself a
  /// Wayland client, else `wayland-N`. [libraryPath] overrides where the
  /// module is loaded from; by default it is the one the build hook bundled.
  static WaylandServer start({String? socketName, String? libraryPath}) {
    final WaylandServer server = _instance ??= WaylandServer._(
      IhsWlBindings(
        libraryPath == null
            ? loadIhsWl()
            : ffi.DynamicLibrary.open(libraryPath),
      ),
    );
    server._start(socketName);
    return server;
  }

  void _start(String? socketName) {
    using((Arena arena) {
      final ffi.Pointer<IhsWlConfig> cfg = arena<IhsWlConfig>();
      cfg.ref.struct_size = ffi.sizeOf<IhsWlConfig>();
      cfg.ref.socket_name = socketName == null
          ? ffi.nullptr
          : socketName.toNativeUtf8(allocator: arena).cast<ffi.Char>();
      final int rc = _lib.ihs_wl_start(cfg);
      if (rc != IhsWlResult.IHS_WL_RESULT_OK.value &&
          rc != IhsWlResult.IHS_WL_RESULT_ERR_ALREADY_RUNNING.value) {
        throw _error('ihs_wl_start', rc);
      }
    });
  }

  /// The socket clients connect to, or null once stopped.
  String? get socketName {
    final ffi.Pointer<ffi.Char> name = _lib.ihs_wl_socket_name();
    return name == ffi.nullptr ? null : name.cast<Utf8>().toDartString();
  }

  /// Disconnect every client and stop the server.
  void stop() => _lib.ihs_wl_stop();

  WaylandServerException _error(String call, int rc) => WaylandServerException(
    call,
    rc,
    _lib.ihs_wl_last_error().cast<Utf8>().toDartString(),
  );
}
