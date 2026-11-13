// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

// Drives the real libihs_wl_server.so over FFI, as hook/build.dart builds it.
// IHS_WL_LIB overrides the library path.

import 'dart:async';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:ihs_wayland_server/ihs_wayland_server.dart';
import 'package:ihs_wayland_server/src/events.dart';

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

    expect(server.clientEnvironment, <String, String>{
      'WAYLAND_DISPLAY': 'ihs-wl-dart-test',
      'XDG_SESSION_TYPE': 'wayland',
    });

    // A second start (hot restart) is not an error.
    WaylandServer.start(socketName: 'ihs-wl-dart-test');

    server.stop();
    expect(server.socketName, isNull);
    expect(server.clientEnvironment, isEmpty);
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

  test('windows: none on a new server, and a query for a gone one', () {
    final WaylandServer server = WaylandServer.start(
      socketName: 'ihs-wl-dart-test-toplevels',
    );
    expect(server.toplevels, isEmpty);
    expect(server.toplevel(1), isNull);
    server.stop();
    // Stopped: nothing to ask.
    expect(server.toplevel(1), isNull);
    expect(() => server.toplevels, throwsA(isA<WaylandServerException>()));
  });

  test('window events reach every listener', () async {
    final WaylandServer server = WaylandServer.start(
      socketName: 'ihs-wl-dart-test-toplevel-events',
    );
    addTearDown(server.stop);
    final List<WaylandToplevelEvent> a = <WaylandToplevelEvent>[];
    final List<WaylandToplevelEvent> b = <WaylandToplevelEvent>[];
    final StreamSubscription<WaylandToplevelEvent> subA = server.toplevelEvents
        .listen(a.add);
    final StreamSubscription<WaylandToplevelEvent> subB = server.toplevelEvents
        .listen(b.add);
    ViewEvents.dispatch(<int>[256, 41]); // mapped, gone before it is asked
    ViewEvents.dispatch(<int>[258, 41]); // unmapped
    await Future<void>.delayed(Duration.zero);
    for (final List<WaylandToplevelEvent> got in <List<WaylandToplevelEvent>>[
      a,
      b,
    ]) {
      expect(
        got.map((WaylandToplevelEvent e) => (e.kind, e.id, e.toplevel)),
        <(WaylandToplevelEventKind, int, WaylandToplevel?)>[
          (WaylandToplevelEventKind.mapped, 41, null),
          (WaylandToplevelEventKind.unmapped, 41, null),
        ],
      );
    }
    await subA.cancel();
    await subB.cancel();
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
