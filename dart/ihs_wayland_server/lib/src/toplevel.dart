// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

import 'package:flutter/foundation.dart';

import 'client.dart';

/// A client window: a toplevel without a parent, whoever launched it.
@immutable
class WaylandToplevel {
  const WaylandToplevel({
    required this.id,
    required this.appId,
    required this.title,
    required this.client,
    this.viewId,
    this.activationToken,
  });

  /// What `WaylandToplevelView.toplevelId` names to show it.
  final int id;

  /// As the client set them; [client] says who it is.
  final String appId;
  final String title;

  /// The platform view showing it, or null while none does.
  final int? viewId;

  /// The token from `WaylandServer.launch` or `activationToken` it was
  /// activated with: the app launched it, for the view naming that token,
  /// which may not be made yet. Null for any other window.
  final String? activationToken;

  final WaylandClient client;

  @override
  bool operator ==(Object other) =>
      other is WaylandToplevel &&
      other.id == id &&
      other.appId == appId &&
      other.title == title &&
      other.viewId == viewId &&
      other.activationToken == activationToken &&
      other.client == client;

  @override
  int get hashCode =>
      Object.hash(id, appId, title, viewId, activationToken, client);

  @override
  String toString() =>
      'WaylandToplevel(id: $id, appId: $appId, title: $title, '
      'viewId: $viewId, activationToken: $activationToken, client: $client)';
}

/// What happened to a window.
enum WaylandToplevelEventKind {
  /// It mapped, or a dialog lost its parent.
  mapped,

  /// Its app_id, title or the view showing it changed.
  changed,

  /// It closed, or became a dialog.
  unmapped,

  /// Its client asks for it to be shown, with a token the focused client
  /// made for user input and still has focus.
  activationRequested,

  /// Its client asks for it to be shown, with any other token.
  attentionRequested,
}

/// Something happened to the window [id].
@immutable
class WaylandToplevelEvent {
  const WaylandToplevelEvent(this.kind, this.id, this.toplevel);

  final WaylandToplevelEventKind kind;
  final int id;

  /// What it is as the event is delivered; null once unmapped, or when it
  /// went before it could be asked.
  final WaylandToplevel? toplevel;

  @override
  String toString() => 'WaylandToplevelEvent($kind, $id, $toplevel)';
}
