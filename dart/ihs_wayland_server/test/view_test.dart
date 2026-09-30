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

  testWidgets('a requested size reaches the view', (WidgetTester tester) async {
    Map<Object?, Object?>? params;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform_views,
      (MethodCall call) async {
        if (call.method == 'create') {
          final Uint8List bytes =
              (call.arguments as Map<Object?, Object?>)['params']! as Uint8List;
          params =
              const StandardMessageCodec().decodeMessage(
                    ByteData.sublistView(bytes),
                  )
                  as Map<Object?, Object?>;
        }
        return null;
      },
    );
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xff000000),
        builder: (BuildContext context, Widget? child) =>
            const WaylandToplevelView(requestedSize: Size(800, 600)),
      ),
    );
    await tester.pump();
    expect(params?['requested_width'], 800.0);
    expect(params?['requested_height'], 600.0);
  });
}
