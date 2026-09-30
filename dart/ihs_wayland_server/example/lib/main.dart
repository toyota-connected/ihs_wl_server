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
//
// The bar above the client shows whether one is shown, scales it to a fixed
// 800x600 (requestedSize), and closes it. The client's own maximize and
// fullscreen buttons hide the bar, and restoring brings it back; its minimize
// button takes the view off screen (the client is suspended) until Restore.

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:ihs_wayland_server/ihs_wayland_server.dart';

const String _appId = String.fromEnvironment('APP_ID');

/// The size the Scale button asks the client for.
const Size _fixedSize = Size(800, 600);

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
      home: ClientPage(socketName: socketName, token: token),
    );
  }
}

class ClientPage extends StatefulWidget {
  const ClientPage({super.key, required this.socketName, this.token});

  final String socketName;
  final String? token;

  @override
  State<ClientPage> createState() => _ClientPageState();
}

class _ClientPageState extends State<ClientPage> {
  late final WaylandToplevelController _controller = WaylandToplevelController(
    windowCapabilities: const <WaylandWindowCapability>{
      WaylandWindowCapability.maximize,
      WaylandWindowCapability.minimize,
      WaylandWindowCapability.fullscreen,
    },
    onWindowRequest: _onWindowRequest,
  );

  /// Maximized or fullscreen: the view fills the window.
  bool _filled = false;
  bool _minimized = false;
  bool _scaled = false;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _onWindowRequest(WaylandWindowRequest request) {
    setState(() {
      switch (request) {
        case WaylandWindowRequest.maximize:
        case WaylandWindowRequest.fullscreen:
          _filled = true;
        case WaylandWindowRequest.unmaximize:
        case WaylandWindowRequest.unfullscreen:
          _filled = false;
        case WaylandWindowRequest.minimize:
          _minimized = true;
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final Widget view = WaylandToplevelView(
      activationToken: widget.token,
      appId: _appId.isEmpty ? null : _appId,
      controller: _controller,
      requestedSize: _scaled ? _fixedSize : null,
    );
    return Scaffold(
      body: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          if (!_filled) _bar(),
          Expanded(
            child: Stack(
              children: <Widget>[
                // Off screen while minimized: the shell suspends the view,
                // and the client is told it is.
                Offstage(offstage: _minimized, child: view),
                if (_minimized)
                  Center(
                    child: FilledButton(
                      onPressed: () => setState(() => _minimized = false),
                      child: const Text('Restore'),
                    ),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _bar() {
    return Padding(
      padding: const EdgeInsets.all(8),
      child: ListenableBuilder(
        listenable: _controller,
        builder: (BuildContext context, Widget? child) {
          return Row(
            children: <Widget>[
              Expanded(
                child: Text(
                  'WAYLAND_DISPLAY=${widget.socketName}  '
                  '${_controller.isBound ? 'client shown' : 'no client'}',
                ),
              ),
              FilterChip(
                label: Text(
                  'Scale ${_fixedSize.width.toInt()}x'
                  '${_fixedSize.height.toInt()}',
                ),
                selected: _scaled,
                onSelected: (bool on) => setState(() => _scaled = on),
              ),
              const SizedBox(width: 8),
              FilledButton.tonal(
                onPressed: _controller.isBound ? _controller.close : null,
                child: const Text('Close'),
              ),
            ],
          );
        },
      ),
    );
  }
}
