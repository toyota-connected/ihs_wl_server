// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

// Drives the real libihs_wl_server.so over FFI, as hook/build.dart builds it.
// IHS_WL_LIB overrides the library path.

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:ihs_wayland_server/ihs_wayland_server.dart';

void main() {
  test('start, socket name, stop, restart', () {
    final WaylandServer server = WaylandServer.start(
      socketName: 'ihs-wl-dart-test',
    );
    expect(server.socketName, 'ihs-wl-dart-test');
    expect(
      File(
        '${Platform.environment['XDG_RUNTIME_DIR']}/ihs-wl-dart-test',
      ).existsSync(),
      isTrue,
    );

    // A second start (hot restart) is not an error.
    WaylandServer.start(socketName: 'ihs-wl-dart-test');

    server.stop();
    expect(server.socketName, isNull);
    server.stop(); // no-op when stopped
  });

  test('activation tokens, and launching a client with one', () async {
    final WaylandServer server = WaylandServer.start(
      socketName: 'ihs-wl-dart-test-token',
    );
    final String a = server.activationToken();
    final String b = server.activationToken();
    expect(a, isNotEmpty);
    expect(a, isNot(b));

    final ({Process process, String token}) launched = await server.launch(
      'sh',
      <String>['-c', r'echo "$WAYLAND_DISPLAY $XDG_ACTIVATION_TOKEN $X"'],
      environment: <String, String>{'X': 'kept'},
      mode: ProcessStartMode.normal,
    );
    final String out = await launched.process.stdout
        .transform(const SystemEncoding().decoder)
        .join();
    expect(await launched.process.exitCode, 0);
    expect(out.trim(), 'ihs-wl-dart-test-token ${launched.token} kept');

    server.stop();
    expect(server.activationToken, throwsA(isA<WaylandServerException>()));
  });

  test('a bad socket name throws with the module error', () {
    expect(
      () => WaylandServer.start(socketName: 'a/b'),
      throwsA(
        isA<WaylandServerException>().having(
          (WaylandServerException e) => e.message,
          'message',
          contains('socket_name'),
        ),
      ),
    );
  });
}
