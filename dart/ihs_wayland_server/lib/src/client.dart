// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

import 'dart:convert';
import 'dart:ffi' as ffi;

import 'package:flutter/foundation.dart';

/// Who a Wayland client is, as the server knows it.
@immutable
class WaylandClient {
  const WaylandClient({
    required this.pid,
    required this.uid,
    required this.gid,
    this.sandbox,
    this.flatpakAppId,
    this.flatpakInstanceId,
  });

  /// The client's credentials as of when it connected.
  final int pid;
  final int uid;
  final int gid;

  /// The sandbox it connected through, or null when it connected to the
  /// server's socket itself.
  final WaylandSandbox? sandbox;

  /// From the client's `/.flatpak-info`, with or without a [sandbox]; null
  /// outside flatpak. A client outside any sandbox can stage its own, so
  /// only [sandbox] vouches for it.
  final String? flatpakAppId;
  final String? flatpakInstanceId;

  @override
  bool operator ==(Object other) =>
      other is WaylandClient &&
      other.pid == pid &&
      other.uid == uid &&
      other.gid == gid &&
      other.sandbox == sandbox &&
      other.flatpakAppId == flatpakAppId &&
      other.flatpakInstanceId == flatpakInstanceId;

  @override
  int get hashCode =>
      Object.hash(pid, uid, gid, sandbox, flatpakAppId, flatpakInstanceId);

  @override
  String toString() =>
      'WaylandClient(pid: $pid, uid: $uid, gid: $gid, sandbox: $sandbox, '
      'flatpakAppId: $flatpakAppId, flatpakInstanceId: $flatpakInstanceId)';
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

/// A NUL-terminated UTF-8 string in [chars]; null when empty.
String? cString(ffi.Array<ffi.Char> chars) {
  final List<int> bytes = <int>[];
  for (int i = 0; i < 256; i++) {
    final int c = chars[i];
    if (c == 0) {
      break;
    }
    bytes.add(c & 0xff);
  }
  return bytes.isEmpty ? null : utf8.decode(bytes, allowMalformed: true);
}
