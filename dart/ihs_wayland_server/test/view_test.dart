// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ihs_wayland_server/ihs_wayland_server.dart';
import 'package:ihs_wayland_server/src/events.dart';

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

  testWidgets('the controller follows the view events', (
    WidgetTester tester,
  ) async {
    int? viewId;
    Map<Object?, Object?>? params;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform_views,
      (MethodCall call) async {
        if (call.method == 'create') {
          final Map<Object?, Object?> args =
              call.arguments as Map<Object?, Object?>;
          viewId = args['id']! as int;
          params =
              const StandardMessageCodec().decodeMessage(
                    ByteData.sublistView(args['params']! as Uint8List),
                  )
                  as Map<Object?, Object?>;
        }
        return null;
      },
    );
    final List<WaylandWindowRequest> requests = <WaylandWindowRequest>[];
    final WaylandToplevelController controller = WaylandToplevelController(
      onWindowRequest: requests.add,
      windowCapabilities: const <WaylandWindowCapability>{
        WaylandWindowCapability.maximize,
        WaylandWindowCapability.fullscreen,
      },
    );
    addTearDown(controller.dispose);
    controller.windowState = const <WaylandWindowState>{
      WaylandWindowState.fullscreen,
    };
    int notified = 0;
    controller.addListener(() => notified++);
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xff000000),
        builder: (BuildContext context, Widget? child) =>
            WaylandToplevelView(controller: controller),
      ),
    );
    await tester.pump();
    final int id = viewId!;
    expect(controller.isBound, isFalse);
    // Maximize (1) and fullscreen (4); granted fullscreen (2).
    expect(params?['capabilities'], 5);
    expect(params?['window_state'], 2);

    ViewEvents.dispatch(<int>[1, id]); // bound
    expect(controller.isBound, isTrue);
    ViewEvents.dispatch(<int>[3, id]); // maximize
    ViewEvents.dispatch(<int>[4, id]); // unmaximize
    ViewEvents.dispatch(<int>[5, id]); // minimize
    ViewEvents.dispatch(<int>[99, id]); // a newer server's event
    ViewEvents.dispatch(<int>[3, id + 1]); // another view
    expect(requests, <WaylandWindowRequest>[
      WaylandWindowRequest.maximize,
      WaylandWindowRequest.unmaximize,
      WaylandWindowRequest.minimize,
    ]);
    ViewEvents.dispatch(<int>[2, id]); // closed
    expect(controller.isBound, isFalse);
    expect(notified, 2);

    // Gone with its view.
    await tester.pumpWidget(const SizedBox());
    ViewEvents.dispatch(<int>[1, id]);
    expect(controller.isBound, isFalse);
  });

  testWidgets('a layout change resizes the platform view', (
    WidgetTester tester,
  ) async {
    final List<MethodCall> calls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform_views,
      (MethodCall call) async {
        calls.add(call);
        return null;
      },
    );
    Widget sized(double width, double height) => WidgetsApp(
      color: const Color(0xff000000),
      builder: (BuildContext context, Widget? child) => Center(
        child: SizedBox(
          width: width,
          height: height,
          child: const WaylandToplevelView(),
        ),
      ),
    );
    await tester.pumpWidget(sized(200, 100));
    await tester.pump();
    final Map<Object?, Object?> create =
        calls.single.arguments as Map<Object?, Object?>;
    expect(calls.single.method, 'create');
    expect((create['width'], create['height']), (200.0, 100.0));

    await tester.pumpWidget(sized(300, 150));
    await tester.pump();
    expect(calls.map((MethodCall c) => c.method), <String>['create', 'resize']);
    expect(calls.last.arguments, <String, Object?>{
      'id': create['id'],
      'width': 300.0,
      'height': 150.0,
    });

    // The same size again sends nothing.
    await tester.pumpWidget(sized(300, 150));
    await tester.pump();
    expect(calls, hasLength(2));
  });
}
