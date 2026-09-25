// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

// Starts the Wayland server and shows the first client toplevel to map, or
// the one with app_id APP_ID:
//
//   flutter build ... --dart-define=APP_ID=weston-simple-egl
//   WAYLAND_DISPLAY=<socket shown on screen> weston-simple-egl

import 'package:flutter/material.dart';
import 'package:ihs_wayland_server/ihs_wayland_server.dart';

const String _appId = String.fromEnvironment('APP_ID');

void main() {
  final WaylandServer server = WaylandServer.start();
  runApp(ExampleApp(socketName: server.socketName ?? ''));
}

class ExampleApp extends StatelessWidget {
  const ExampleApp({super.key, required this.socketName});

  final String socketName;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      home: Scaffold(
        body: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            Padding(
              padding: const EdgeInsets.all(8),
              child: Text('WAYLAND_DISPLAY=$socketName'),
            ),
            Expanded(
              child: WaylandToplevelView(appId: _appId.isEmpty ? null : _appId),
            ),
          ],
        ),
      ),
    );
  }
}
