// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

// A view an ancestor draws scaled is sized, and takes input, as it is on
// screen. Input goes to a fake module: Dart callbacks behind the FFI
// symbols.

import 'dart:ffi' as ffi;

import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ihs_wayland_server/ihs_wayland_server.dart';
import 'package:ihs_wayland_server/src/bindings.g.dart';
import 'package:ihs_wayland_server/src/input.dart';
import 'package:ihs_wayland_server/src/server.dart';

/// What the fake module was sent.
class _Module {
  _Module() {
    _add(
      'ihs_wl_pointer',
      ffi.NativeCallable<
        ffi.Int Function(ffi.Int32, ffi.Pointer<IhsWlPointerEvent>)
      >.isolateLocal(_pointer, exceptionalReturn: -1),
    );
    _add(
      'ihs_wl_touch',
      ffi.NativeCallable<
        ffi.Int Function(ffi.Int32, ffi.Pointer<IhsWlTouchEvent>)
      >.isolateLocal(_touch, exceptionalReturn: -1),
    );
    _add(
      'ihs_wl_view_size',
      ffi.NativeCallable<
        ffi.Int Function(ffi.Int32, ffi.Double, ffi.Double)
      >.isolateLocal(_ok3, exceptionalReturn: -1),
    );
    for (final String name in <String>[
      'ihs_wl_focus',
      'ihs_wl_view_capabilities',
      'ihs_wl_view_state',
    ]) {
      _add(
        name,
        ffi.NativeCallable<
          ffi.Int Function(ffi.Int32, ffi.Uint32)
        >.isolateLocal(_ok2, exceptionalReturn: -1),
      );
    }
  }

  final Map<String, ffi.Pointer<ffi.Void>> _symbols =
      <String, ffi.Pointer<ffi.Void>>{};
  final List<void Function()> _closers = <void Function()>[];

  /// Pointer motion: (x, y).
  final List<Offset> motion = <Offset>[];

  /// Scroll: (axis_x, axis_y).
  final List<Offset> axis = <Offset>[];

  /// Touch down and motion: (x, y).
  final List<Offset> touch = <Offset>[];

  void _add<T extends Function>(String name, ffi.NativeCallable<T> callable) {
    _symbols[name] = callable.nativeFunction.cast();
    _closers.add(callable.close);
  }

  IhsWlBindings get bindings => IhsWlBindings.fromLookup(
    <T extends ffi.NativeType>(String name) =>
        (_symbols[name] ?? (throw ArgumentError('no fake $name'))).cast<T>(),
  );

  void close() {
    for (final void Function() close in _closers) {
      close();
    }
  }

  int _pointer(int view, ffi.Pointer<IhsWlPointerEvent> p) {
    final IhsWlPointerEvent ev = p.ref;
    if (ev.kind == IhsWlPointerKind.IHS_WL_POINTER_KIND_MOTION.value) {
      motion.add(Offset(ev.x, ev.y));
    } else if (ev.kind == IhsWlPointerKind.IHS_WL_POINTER_KIND_AXIS.value) {
      axis.add(Offset(ev.axis_x, ev.axis_y));
    }
    return -1;
  }

  int _touch(int view, ffi.Pointer<IhsWlTouchEvent> p) {
    final IhsWlTouchEvent ev = p.ref;
    if (ev.kind == IhsWlTouchKind.IHS_WL_TOUCH_KIND_DOWN.value ||
        ev.kind == IhsWlTouchKind.IHS_WL_TOUCH_KIND_MOTION.value) {
      touch.add(Offset(ev.x, ev.y));
    }
    return 0;
  }

  static int _ok2(int view, int value) => 0;
  static int _ok3(int view, double a, double b) => 0;
}

void main() {
  late _Module module;
  late List<MethodCall> calls;

  setUp(() {
    module = _Module();
    debugWaylandInput = WaylandInput(module.bindings);
  });

  tearDown(() {
    debugWaylandInput = null;
    module.close();
  });

  void mockPlatformViews(WidgetTester tester) {
    calls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform_views,
      (MethodCall call) async {
        calls.add(call);
        return null;
      },
    );
  }

  (double, double) sizeOf(MethodCall call) {
    final Map<Object?, Object?> args = call.arguments as Map<Object?, Object?>;
    return (args['width']! as double, args['height']! as double);
  }

  /// A 200x100 view, drawn into [width]x[height] on screen.
  Widget fitted(double width, double height) => WidgetsApp(
    color: const Color(0xff000000),
    builder: (BuildContext context, Widget? child) => Center(
      child: SizedBox(
        width: width,
        height: height,
        child: const FittedBox(
          fit: BoxFit.fill,
          child: SizedBox(
            width: 200,
            height: 100,
            child: WaylandToplevelView(),
          ),
        ),
      ),
    ),
  );

  Future<void> expectInput(WidgetTester tester, Offset at) async {
    final Offset origin = tester.getTopLeft(find.byType(WaylandToplevelView));

    final TestGesture mouse = await tester.createGesture(
      kind: PointerDeviceKind.mouse,
    );
    await mouse.addPointer(location: origin + at);
    await mouse.moveTo(origin + at + const Offset(1, 0));
    await mouse.removePointer();
    expect(module.motion.last, at + const Offset(1, 0));

    final TestGesture finger = await tester.startGesture(origin + at);
    await finger.up();
    expect(module.touch.single, at);

    tester.binding.handlePointerEvent(
      PointerScrollEvent(
        position: origin + at,
        scrollDelta: const Offset(10, 20),
      ),
    );
    expect(module.axis.single, const Offset(10, 20));
  }

  testWidgets('scaled unevenly: created, and takes input, as on screen', (
    WidgetTester tester,
  ) async {
    mockPlatformViews(tester);
    await tester.pumpWidget(fitted(400, 300));
    await tester.pump();
    expect(calls.single.method, 'create');
    expect(sizeOf(calls.single), (400.0, 300.0));

    // On screen, not in the view's layout: (20, 60) is (10, 20) there.
    await expectInput(tester, const Offset(20, 60));
  });

  testWidgets('a new scale resizes the view', (WidgetTester tester) async {
    mockPlatformViews(tester);
    await tester.pumpWidget(fitted(400, 300));
    await tester.pump();
    await tester.pumpWidget(fitted(100, 100));
    await tester.pump();
    expect(calls.map((MethodCall c) => c.method), <String>['create', 'resize']);
    expect(sizeOf(calls.last), (100.0, 100.0));
  });

  testWidgets('a transform that changes without repainting the view', (
    WidgetTester tester,
  ) async {
    mockPlatformViews(tester);
    Widget scaled(double scale) => WidgetsApp(
      color: const Color(0xff000000),
      builder: (BuildContext context, Widget? child) => Center(
        child: Transform.scale(
          scale: scale,
          child: const RepaintBoundary(
            child: SizedBox(
              width: 200,
              height: 100,
              child: WaylandToplevelView(),
            ),
          ),
        ),
      ),
    );
    await tester.pumpWidget(scaled(1));
    await tester.pump();
    expect(sizeOf(calls.single), (200.0, 100.0));

    await tester.pumpWidget(scaled(1.5));
    await tester.pump();
    expect(calls.map((MethodCall c) => c.method), <String>['create', 'resize']);
    expect(sizeOf(calls.last), (300.0, 150.0));
  });

  testWidgets('unscaled: as laid out', (WidgetTester tester) async {
    mockPlatformViews(tester);
    await tester.pumpWidget(fitted(200, 100));
    await tester.pump();
    expect(sizeOf(calls.single), (200.0, 100.0));
    await expectInput(tester, const Offset(20, 60));
  });
}
