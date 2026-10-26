// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

import 'package:flutter/foundation.dart';

/// Who a Wayland client is, as the server knows it.
@immutable
class WaylandClient {
  const WaylandClient({
    required this.pid,
    required this.uid,
    required this.gid,
    this.sandbox,
  });

  /// The client's credentials as of when it connected.
  final int pid;
  final int uid;
  final int gid;

  /// The sandbox it connected through, or null when it connected to the
  /// server's socket itself.
  final WaylandSandbox? sandbox;

  @override
  bool operator ==(Object other) =>
      other is WaylandClient &&
      other.pid == pid &&
      other.uid == uid &&
      other.gid == gid &&
      other.sandbox == sandbox;

  @override
  int get hashCode => Object.hash(pid, uid, gid, sandbox);

  @override
  String toString() =>
      'WaylandClient(pid: $pid, uid: $uid, gid: $gid, sandbox: $sandbox)';
}

/// A sandbox engine's identity for a client (wp_security_context_v1): set
/// by the engine, such as flatpak, which the client cannot change.
@immutable
class WaylandSandbox {
  const WaylandSandbox({this.engine, this.appId, this.instanceId});

  /// The sandbox engine, e.g. `org.flatpak`.
  final String? engine;

  /// The app within the engine, e.g. a flatpak app id.
  final String? appId;

  /// The running instance of the app.
  final String? instanceId;

  @override
  bool operator ==(Object other) =>
      other is WaylandSandbox &&
      other.engine == engine &&
      other.appId == appId &&
      other.instanceId == instanceId;

  @override
  int get hashCode => Object.hash(engine, appId, instanceId);

  @override
  String toString() =>
      'WaylandSandbox(engine: $engine, appId: $appId, '
      'instanceId: $instanceId)';
}
