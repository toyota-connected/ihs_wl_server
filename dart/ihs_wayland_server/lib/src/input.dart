// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

import 'dart:ffi' as ffi;

import 'package:ffi/ffi.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';

import 'bindings.g.dart';

/// Linux `BTN_*` codes for Flutter's mouse button bits.
const Map<int, int> _buttonCodes = <int, int>{
  kPrimaryButton: 0x110, // BTN_LEFT
  kSecondaryButton: 0x111, // BTN_RIGHT
  kMiddleMouseButton: 0x112, // BTN_MIDDLE
  kBackMouseButton: 0x113, // BTN_SIDE
  kForwardMouseButton: 0x114, // BTN_EXTRA
};

/// The buttons that went down (true) or up between two `buttons` masks, as
/// `BTN_*` codes.
List<(int, bool)> buttonChanges(int before, int after) => <(int, bool)>[
  for (final MapEntry<int, int> b in _buttonCodes.entries)
    if ((before ^ after) & b.key != 0) (b.value, after & b.key != 0),
];

/// xkb keycodes are evdev codes offset by 8.
const int _xkbOffset = 8;

/// USB HID usage to evdev code: Flutter's own Linux table, inverted.
final Map<int, int> _evdevByHid = <int, int>{
  for (final MapEntry<int, PhysicalKeyboardKey> e
      in kLinuxToPhysicalKey.entries)
    e.value.usbHidUsage: e.key - _xkbOffset,
};

/// The evdev code of [key], or null for a key Linux has none for.
int? evdevCode(PhysicalKeyboardKey key) => _evdevByHid[key.usbHidUsage];

/// Feeds one view's input to the module. The event structs are allocated
/// once and reused: input only ever comes from the UI thread, and the
/// module copies each event before it returns.
class WaylandInput {
  WaylandInput(this._lib);

  final IhsWlBindings _lib;

  late final ffi.Pointer<IhsWlPointerEvent> _pointer = () {
    final ffi.Pointer<IhsWlPointerEvent> p = calloc<IhsWlPointerEvent>();
    p.ref.struct_size = ffi.sizeOf<IhsWlPointerEvent>();
    return p;
  }();

  late final ffi.Pointer<IhsWlTouchEvent> _touch = () {
    final ffi.Pointer<IhsWlTouchEvent> p = calloc<IhsWlTouchEvent>();
    p.ref.struct_size = ffi.sizeOf<IhsWlTouchEvent>();
    return p;
  }();

  /// Mouse buttons held, by Flutter device.
  final Map<int, int> _buttons = <int, int>{};

  /// A pointer event the platform view received.
  void pointerEvent(int viewId, PointerEvent event) {
    switch (event.kind) {
      case PointerDeviceKind.touch:
      case PointerDeviceKind.stylus:
      case PointerDeviceKind.invertedStylus:
        _touchEvent(viewId, event);
      case PointerDeviceKind.mouse:
      case PointerDeviceKind.trackpad:
      case PointerDeviceKind.unknown:
        _mouseEvent(viewId, event);
    }
  }

  void _mouseEvent(int viewId, PointerEvent event) {
    final int before = _buttons[event.device] ?? 0;
    final int after = event is PointerUpEvent || event is PointerCancelEvent
        ? 0
        : event.buttons;
    if (event is PointerHoverEvent || event is PointerMoveEvent) {
      _sendPointer(viewId, IhsWlPointerKind.IHS_WL_POINTER_KIND_MOTION, event);
    }
    for (final (int code, bool pressed) in buttonChanges(before, after)) {
      final IhsWlPointerEvent ev = _pointer.ref;
      ev.button = code;
      ev.pressed = pressed ? 1 : 0;
      _sendPointer(viewId, IhsWlPointerKind.IHS_WL_POINTER_KIND_BUTTON, event);
    }
    if (after == 0) {
      _buttons.remove(event.device);
    } else {
      _buttons[event.device] = after;
    }
  }

  /// Scroll over the view, in logical px, positive down and right.
  void scroll(int viewId, PointerScrollEvent event) {
    final IhsWlPointerEvent ev = _pointer.ref;
    ev.axis_source = event.kind == PointerDeviceKind.trackpad
        ? 1 // finger
        : 0; // wheel
    ev.axis_x = event.scrollDelta.dx;
    ev.axis_y = event.scrollDelta.dy;
    _sendPointer(viewId, IhsWlPointerKind.IHS_WL_POINTER_KIND_AXIS, event);
    ev.axis_x = 0;
    ev.axis_y = 0;
  }

  /// The pointer left the view.
  void leave(int viewId, PointerEvent event) =>
      _sendPointer(viewId, IhsWlPointerKind.IHS_WL_POINTER_KIND_LEAVE, event);

  void _sendPointer(int viewId, IhsWlPointerKind kind, PointerEvent event) {
    final IhsWlPointerEvent ev = _pointer.ref;
    ev.kind = kind.value;
    ev.x = event.localPosition.dx;
    ev.y = event.localPosition.dy;
    ev.time_us = event.timeStamp.inMicroseconds;
    _lib.ihs_wl_pointer(viewId, _pointer);
  }

  void _touchEvent(int viewId, PointerEvent event) {
    final IhsWlTouchKind kind;
    switch (event) {
      case PointerDownEvent():
        kind = IhsWlTouchKind.IHS_WL_TOUCH_KIND_DOWN;
      case PointerMoveEvent():
        kind = IhsWlTouchKind.IHS_WL_TOUCH_KIND_MOTION;
      case PointerUpEvent():
        kind = IhsWlTouchKind.IHS_WL_TOUCH_KIND_UP;
      case PointerCancelEvent():
        _sendTouch(viewId, IhsWlTouchKind.IHS_WL_TOUCH_KIND_CANCEL, event);
        return;
      default:
        return;
    }
    _sendTouch(viewId, kind, event);
    // Flutter delivers each contact on its own: every event is a frame.
    _sendTouch(viewId, IhsWlTouchKind.IHS_WL_TOUCH_KIND_FRAME, event);
  }

  void _sendTouch(int viewId, IhsWlTouchKind kind, PointerEvent event) {
    final IhsWlTouchEvent ev = _touch.ref;
    ev.kind = kind.value;
    ev.slot = event.pointer & 0x7fffffff;
    ev.x = event.localPosition.dx;
    ev.y = event.localPosition.dy;
    ev.time_us = event.timeStamp.inMicroseconds;
    _lib.ihs_wl_touch(viewId, _touch);
  }

  /// A key event while the view has focus. Returns false for a key the
  /// client cannot be sent.
  bool key(int viewId, KeyEvent event) {
    // Clients repeat keys themselves, from wl_keyboard.repeat_info.
    if (event is KeyRepeatEvent) {
      return true;
    }
    final int? code = evdevCode(event.physicalKey);
    if (code == null) {
      return false;
    }
    _lib.ihs_wl_key(
      viewId,
      code,
      event is KeyDownEvent ? 1 : 0,
      event.timeStamp.inMicroseconds,
    );
    return true;
  }

  /// The view gained or lost keyboard focus.
  void focus(int viewId, bool focused) =>
      _lib.ihs_wl_focus(viewId, focused ? 1 : 0);
}
