// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

// Finds libihs_wl_server.so, which hook/build.dart builds and the app bundles.
//
// In order:
//   1. IHS_WL_LIB
//   2. dlopen by name: the embedder's RUNPATH, ld.so.cache
//   3. beside a loaded libapp.so / libflutter_*.so (/proc/self/maps), or in
//      the bundle's flutter_assets/native_assets/linux/: ivi-homescreen lives
//      outside the bundle, so 2 misses it
//   4. the hook's cargo output under .dart_tool/hooks_runner, newest first:
//      flutter test, dart run

import 'dart:ffi';
import 'dart:io';

const _libName = 'libihs_wl_server.so';

DynamicLibrary loadIhsWl() {
  final env = Platform.environment['IHS_WL_LIB'];
  if (env != null && env.isNotEmpty) return DynamicLibrary.open(env);

  final errors = <String>[];
  try {
    return DynamicLibrary.open(_libName);
  } on ArgumentError catch (e) {
    errors.add('$_libName: $e');
  }

  for (final path in [?_besideLoadedBundle(), ?_newestHookBuild()]) {
    try {
      return DynamicLibrary.open(path);
    } on ArgumentError catch (e) {
      errors.add('$path: $e');
    }
  }
  throw StateError(
    'cannot load $_libName; set IHS_WL_LIB\n${errors.join('\n')}',
  );
}

String? _besideLoadedBundle() {
  try {
    final dirs = <String>{};
    for (final line in File('/proc/self/maps').readAsLinesSync()) {
      final path = line.substring(line.lastIndexOf(' ') + 1);
      final slash = path.lastIndexOf('/');
      if (!path.startsWith('/') || slash <= 0) continue;
      final base = path.substring(slash + 1);
      if (base == 'libapp.so' || base.startsWith('libflutter_')) {
        dirs.add(path.substring(0, slash));
      }
    }
    for (final dir in dirs) {
      // lib/ itself, or where `flutter build bundle` leaves native assets:
      // <bundle>/data/flutter_assets/native_assets/linux/.
      for (final candidate in [
        File('$dir/$_libName'),
        File('$dir/../data/flutter_assets/native_assets/linux/$_libName'),
      ]) {
        if (candidate.existsSync()) return candidate.absolute.path;
      }
    }
  } on FileSystemException {
    // no /proc
  }
  return null;
}

String? _newestHookBuild() {
  var dir = Directory.current;
  for (var i = 0; i < 6; i++) {
    final root = Directory(
      '${dir.path}/.dart_tool/hooks_runner/shared/ihs_wayland_server',
    );
    if (root.existsSync()) {
      File? newest;
      for (final f in root.listSync(recursive: true).whereType<File>()) {
        if (!f.path.endsWith('/release/$_libName')) continue;
        if (newest == null ||
            f.lastModifiedSync().isAfter(newest.lastModifiedSync())) {
          newest = f;
        }
      }
      if (newest != null) return newest.path;
    }
    if (dir.parent.path == dir.path) break;
    dir = dir.parent;
  }
  return null;
}
