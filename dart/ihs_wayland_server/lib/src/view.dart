import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

/// The platform-view type the module's factory registers.
const String _viewType = 'ihs_wl/toplevel';

/// Shows one Wayland client toplevel.
///
/// The toplevel is found by [activationToken] (the token the launcher put in
/// the client's `XDG_ACTIVATION_TOKEN`), or else by [appId]: the oldest
/// toplevel with that app_id not already shown elsewhere. Until a matching
/// client maps, the view is empty.
class WaylandToplevelView extends StatelessWidget {
  const WaylandToplevelView({
    super.key,
    this.activationToken,
    this.appId,
  }) : assert(activationToken != null || appId != null,
            'a view needs an activation token or an app_id to bind by');

  final String? activationToken;
  final String? appId;

  @override
  Widget build(BuildContext context) {
    final double dpr = MediaQuery.devicePixelRatioOf(context);
    return PlatformViewLink(
      viewType: _viewType,
      surfaceFactory:
          (BuildContext context, PlatformViewController controller) {
        return PlatformViewSurface(
          controller: controller,
          gestureRecognizers: const <Factory<OneSequenceGestureRecognizer>>{
            Factory<OneSequenceGestureRecognizer>(EagerGestureRecognizer.new),
          },
          hitTestBehavior: PlatformViewHitTestBehavior.opaque,
        );
      },
      onCreatePlatformView: (PlatformViewCreationParams params) {
        // PlatformViewLink calls create(size:) once laid out, so the view
        // is created at its real size rather than 0x0.
        return _ToplevelViewController(
          id: params.id,
          creationParams: <String, Object?>{
            'token': activationToken,
            'app_id': appId,
            'dpr': dpr,
          },
          onCreated: params.onPlatformViewCreated,
        );
      },
    );
  }
}

class _ToplevelViewController extends PlatformViewController {
  _ToplevelViewController({
    required this.id,
    required this.creationParams,
    required this.onCreated,
  });

  final int id;
  final Map<String, Object?> creationParams;
  final PlatformViewCreatedCallback onCreated;

  bool _created = false;
  Future<void>? _creation;

  @override
  int get viewId => id;

  @override
  bool get awaitingCreation => !_created;

  @override
  Future<void> create({Size? size, Offset? position}) =>
      _creation ??= _createOnce(size);

  Future<void> _createOnce(Size? size) async {
    final ByteData? params =
        const StandardMessageCodec().encodeMessage(creationParams);
    await SystemChannels.platform_views.invokeMethod<void>(
      'create',
      <String, Object?>{
        'id': id,
        'viewType': _viewType,
        'direction': 0,
        'width': size?.width ?? 0.0,
        'height': size?.height ?? 0.0,
        if (params != null)
          'params': params.buffer
              .asUint8List(params.offsetInBytes, params.lengthInBytes),
      },
    );
    _created = true;
    onCreated(id);
  }

  // Input is to go straight to the module over FFI (ihs_wl_pointer and
  // friends), which do not accept events yet.
  @override
  Future<void> dispatchPointerEvent(PointerEvent event) async {}

  @override
  Future<void> clearFocus() async {}

  @override
  Future<void> dispose() async {
    if (!_created) {
      return;
    }
    await SystemChannels.platform_views
        .invokeMethod<void>('dispose', <String, Object>{'id': id});
  }
}
