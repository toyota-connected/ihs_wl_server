// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ihs_wayland_server/src/input.dart';

void main() {
  test('physical keys map to evdev codes', () {
    expect(evdevCode(PhysicalKeyboardKey.escape), 1); // KEY_ESC
    expect(evdevCode(PhysicalKeyboardKey.keyA), 30); // KEY_A
    expect(evdevCode(PhysicalKeyboardKey.enter), 28); // KEY_ENTER
    expect(evdevCode(PhysicalKeyboardKey.shiftLeft), 42); // KEY_LEFTSHIFT
    expect(evdevCode(PhysicalKeyboardKey.arrowUp), 103); // KEY_UP
    expect(evdevCode(PhysicalKeyboardKey.fn), isNull);
  });

  test('button masks become BTN_* presses and releases', () {
    expect(buttonChanges(0, kPrimaryButton), <(int, bool)>[(0x110, true)]);
    expect(buttonChanges(kPrimaryButton, 0), <(int, bool)>[(0x110, false)]);
    expect(
      buttonChanges(kPrimaryButton, kSecondaryButton | kMiddleMouseButton),
      <(int, bool)>[(0x110, false), (0x111, true), (0x112, true)],
    );
    expect(buttonChanges(kBackMouseButton, kBackMouseButton), isEmpty);
  });

  test('cursor shapes map to Flutter cursors', () {
    expect(cursorFor(0), SystemMouseCursors.basic); // none asked
    expect(cursorFor(1), SystemMouseCursors.basic); // default
    expect(cursorFor(4), SystemMouseCursors.click); // pointer
    expect(cursorFor(9), SystemMouseCursors.text);
    expect(cursorFor(29), SystemMouseCursors.resizeUpLeftDownRight); // nwse
    expect(cursorFor(34), SystemMouseCursors.zoomOut);
    expect(cursorFor(99), SystemMouseCursors.basic);
    expect(cursorFor(cursorHidden), SystemMouseCursors.none);
  });
}
