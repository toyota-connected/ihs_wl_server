// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ihs_wayland_server/ihs_wayland_server.dart';

void main() {
  testWidgets('a press takes focus, a press elsewhere gives it up', (
    WidgetTester tester,
  ) async {
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform_views,
      (MethodCall call) async => null,
    );
    final FocusNode focus = FocusNode();
    addTearDown(focus.dispose);
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xff000000),
        builder: (BuildContext context, Widget? child) => Column(
          children: <Widget>[
            const SizedBox(height: 50, child: Text('label')),
            Expanded(child: WaylandToplevelView(focusNode: focus)),
          ],
        ),
      ),
    );

    await tester.tap(find.byType(WaylandToplevelView));
    await tester.pump();
    expect(focus.hasFocus, isTrue);

    // Nothing at the label takes focus, yet the view gives it up.
    await tester.tap(find.text('label'));
    await tester.pump();
    expect(focus.hasFocus, isFalse);
  });
}
