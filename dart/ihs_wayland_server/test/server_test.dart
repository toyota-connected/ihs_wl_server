// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

// Drives the real libihs_wl_server.so over FFI. Build it first:
//   cargo build            (from the repo root)
// IHS_WL_LIB overrides the library path.

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:ihs_wayland_server/ihs_wayland_server.dart';

void main() {
  final String lib = Platform.environment['IHS_WL_LIB'] ??
      '../../target/debug/libihs_wl_server.so';

  test('start, socket name, stop, restart', () {
    final WaylandServer server =
        WaylandServer.start(socketName: 'ihs-wl-dart-test', libraryPath: lib);
    expect(server.socketName, 'ihs-wl-dart-test');
    expect(
      File('${Platform.environment['XDG_RUNTIME_DIR']}/ihs-wl-dart-test')
          .existsSync(),
      isTrue,
    );

    // A second start (hot restart) is not an error.
    WaylandServer.start(socketName: 'ihs-wl-dart-test', libraryPath: lib);

    server.stop();
    expect(server.socketName, isNull);
    server.stop(); // no-op when stopped
  });

  test('a bad socket name throws with the module error', () {
    expect(
      () => WaylandServer.start(socketName: 'a/b', libraryPath: lib),
      throwsA(isA<WaylandServerException>().having(
          (WaylandServerException e) => e.message,
          'message',
          contains('socket_name'))),
    );
  });
}
