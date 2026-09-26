// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

// Builds libihs_wl_server.so from the crate at the repository root with cargo
// and bundles it as this package's code asset.
//
// Hooks run with a filtered environment: PATH and HOME pass, PKG_CONFIG_* does
// not, and CARGO_* only with newer hooks runners. So:
//   - a cross build puts a `cargo` wrapper first on PATH that sets the target,
//     CARGO_HOME, PKG_CONFIG_* and the like (meta-flutter does); without one
//     the target is <arch>-unknown-linux-gnu from the hook config, unless
//     CARGO_BUILD_TARGET gets through
//   - ivi-homescreen-shared.pc outside the default search path is named by the
//     app's pubspec:
//
//       hooks:
//         user_defines:
//           ihs_wayland_server:
//             pkg_config_path: /path/to/prefix/lib/pkgconfig
//
//   - cargo features the same way, as a list or a comma-separated string:
//
//             cargo_features: [egl-wl-display]
//
//   - a library installed by the system (a Yocto image's ihs-wl-server
//     package, say) rather than built here:
//
//             system_library: true
//
//     Nothing is built or bundled then; the app loads libihs_wl_server.so by
//     name, from the linker's search path.
//
// SKIP_NATIVE_BUILD skips the build too, where the environment gets through.

import 'dart:convert';
import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:hooks/hooks.dart';

const _crate = 'ihs_wl_server';

void main(List<String> args) async {
  await build(args, (input, output) async {
    if (!input.config.buildCodeAssets) return;
    if (Platform.environment.containsKey('SKIP_NATIVE_BUILD')) return;
    if (_flag(input.userDefines['system_library'], 'system_library')) return;

    final code = input.config.code;
    if (code.targetOS != OS.linux) {
      throw UnsupportedError('$_crate is Linux only, not ${code.targetOS}');
    }

    final crateDir = input.packageRoot.resolve('../../');
    final manifest = File.fromUri(crateDir.resolve('Cargo.toml'));
    if (!manifest.existsSync()) {
      throw StateError('no Cargo.toml at ${manifest.path}');
    }

    final env = <String, String>{};
    final pkgConfigPath = input.userDefines.path('pkg_config_path');
    if (pkgConfigPath != null) {
      env['PKG_CONFIG_PATH'] = pkgConfigPath.toFilePath();
    }
    if (!Platform.environment.containsKey('CARGO_TARGET_DIR')) {
      env['CARGO_TARGET_DIR'] = input.outputDirectoryShared
          .resolve('target/')
          .toFilePath();
    }

    final features = _features(input.userDefines['cargo_features']);

    final cargoArgs = [
      'build',
      '--release',
      '--lib',
      '--locked',
      '--message-format=json-render-diagnostics',
      '--manifest-path',
      manifest.path,
      if (features.isNotEmpty) ...['--features', features.join(',')],
      if (!Platform.environment.containsKey('CARGO_BUILD_TARGET')) ...[
        '--target',
        _triple(code.targetArchitecture),
      ],
    ];

    final process = await Process.start(
      'cargo',
      cargoArgs,
      environment: env,
      workingDirectory: crateDir.toFilePath(),
    );
    process.stderr.listen(stderr.add);

    // The artifact's path depends on the target and the target dir; take it
    // from cargo rather than guessing.
    Uri? library;
    await for (final line
        in process.stdout
            .transform(utf8.decoder)
            .transform(const LineSplitter())) {
      if (!line.startsWith('{')) continue;
      final msg = jsonDecode(line) as Map<String, dynamic>;
      if (msg['reason'] != 'compiler-artifact') continue;
      final target = msg['target'] as Map<String, dynamic>;
      if (target['name'] != _crate) continue;
      for (final f in (msg['filenames'] as List).cast<String>()) {
        if (f.endsWith('.so')) library = Uri.file(f);
      }
    }
    final rc = await process.exitCode;
    if (rc != 0) {
      throw ProcessException('cargo', cargoArgs, 'exit code $rc', rc);
    }
    if (library == null) {
      throw StateError('cargo built no lib$_crate.so');
    }

    output.assets.code.add(
      CodeAsset(
        package: input.packageName,
        name: 'src/bindings.g.dart',
        linkMode: DynamicLoadingBundled(),
        file: library,
      ),
    );

    output.dependencies.addAll([
      manifest.uri,
      crateDir.resolve('Cargo.lock'),
      crateDir.resolve('build.rs'),
      crateDir.resolve('rust-toolchain.toml'),
      for (final dir in ['src/', 'include/'])
        ...Directory.fromUri(
          crateDir.resolve(dir),
        ).listSync(recursive: true).whereType<File>().map((f) => f.uri),
    ]);
  });
}

String _triple(Architecture arch) => switch (arch) {
  Architecture.arm64 => 'aarch64-unknown-linux-gnu',
  Architecture.x64 => 'x86_64-unknown-linux-gnu',
  Architecture.arm => 'armv7-unknown-linux-gnueabihf',
  Architecture.riscv64 => 'riscv64gc-unknown-linux-gnu',
  _ => throw UnsupportedError('no Rust target for $arch'),
};

/// The `cargo_features` user-define: a list of names, or one string of
/// comma-separated names.
List<String> _features(Object? value) {
  final names = switch (value) {
    null => const <String>[],
    final String s => s.split(','),
    final List<Object?> l => [for (final e in l) '$e'],
    _ => throw ArgumentError.value(
      value,
      'cargo_features',
      'a list or a comma-separated string',
    ),
  };
  final valid = RegExp(r'^[A-Za-z0-9_-]+$');
  return [
    for (final raw in names)
      if (raw.trim().isNotEmpty)
        valid.hasMatch(raw.trim())
            ? raw.trim()
            : throw ArgumentError.value(
                raw,
                'cargo_features',
                'not a cargo feature name',
              ),
  ];
}

/// A boolean user-define: absent is false.
bool _flag(Object? value, String name) => switch (value) {
  null => false,
  bool b => b,
  _ => throw ArgumentError.value(value, name, 'expected true or false'),
};
