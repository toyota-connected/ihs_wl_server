// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

// Starts the Wayland server and shows the first client toplevel to map, or
// the one with app_id APP_ID:
//
//   flutter build ... --dart-define=APP_ID=weston-simple-egl
//   WAYLAND_DISPLAY=<socket shown on screen> weston-simple-egl
//
// Or, with LAUNCH in the shell's environment, launches that client itself
// and shows its toplevel, found by activation token:
//
//   LAUNCH=gnome-calculator homescreen ...

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:ihs_wayland_server/ihs_wayland_server.dart';

const String _appId = String.fromEnvironment('APP_ID');
Future<void> main() async {
  final WaylandServer server = WaylandServer.start();
  final String launch = Platform.environment['LAUNCH'] ?? '';
  String? token;
  if (launch.isNotEmpty) {
    token = (await server.launch(launch, <String>[])).token;
  }
  runApp(ExampleApp(socketName: server.socketName ?? '', token: token));
}

class ExampleApp extends StatelessWidget {
  const ExampleApp({super.key, required this.socketName, this.token});

  final String socketName;
  final String? token;

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
              child: WaylandToplevelView(
                activationToken: token,
                appId: _appId.isEmpty ? null : _appId,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
