#!/usr/bin/env python3
"""Stage or verify the versioned, optimized SwiftPM SDK."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SDK = ROOT / 'sdk/swift'
TARGETS = {
    ('ios', None, 'arm64'): 'aarch64-apple-ios',
    ('ios', 'simulator', 'arm64'): 'aarch64-apple-ios-sim',
    ('ios', 'simulator', 'x86_64'): 'x86_64-apple-ios',
    ('macos', None, 'arm64'): 'aarch64-apple-darwin',
    ('macos', None, 'x86_64'): 'x86_64-apple-darwin',
}


def checksum(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for part in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(part)
    return digest.hexdigest()


def source_digest():
    digest = hashlib.sha256()
    files = sorted((ROOT / 'src').rglob('*.rs')) + [ROOT / name for name in
        ['Cargo.toml', 'Cargo.lock', 'boltffi.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'scripts/build-sdk.sh']] + sorted((ROOT / 'sdk/swift-support').glob('*.swift'))
    for path in files:
        digest.update(str(path.relative_to(ROOT)).encode() + b'\0' + path.read_bytes() + b'\0')
    return digest.hexdigest()


def artifacts():
    return {str(p.relative_to(SDK)): checksum(p) for p in sorted(SDK.rglob('*'))
            if p.is_file() and p.name != 'provenance.json' and p.name != '.DS_Store'}


def check_build(pr_build=False):
    targets = {key: target for key, target in TARGETS.items()
               if not pr_build or key[2] == 'arm64'}
    generated = ROOT / 'dist/apple'
    framework = generated / 'BarkBoltFFI.xcframework'
    info = plistlib.loads((framework / 'Info.plist').read_bytes())
    target_dir = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
    seen = set()
    with tempfile.TemporaryDirectory(prefix='bark-swift-check-') as directory:
        for library in info['AvailableLibraries']:
            binary = framework / library['LibraryIdentifier'] / library['LibraryPath']
            architectures = library['SupportedArchitectures']
            for arch in architectures:
                key = (library['SupportedPlatform'], library.get('SupportedPlatformVariant'), arch)
                assert key in targets, f'Unexpected Apple slice: {key}'
                thin = binary
                if len(architectures) > 1:
                    thin = Path(directory) / ('-'.join(str(v) for v in key) + '.a')
                    subprocess.run(['xcrun', 'lipo', str(binary), '-thin', arch, '-output', str(thin)], check=True)
                commands = subprocess.check_output(['xcrun', 'otool', '-arch', 'all', '-l', str(thin)], text=True)
                minimums = []
                command = ''
                for line in commands.splitlines():
                    parts = line.split()
                    if len(parts) == 2 and parts[0] == 'cmd':
                        command = parts[1]
                    if len(parts) == 2 and ((command == 'LC_BUILD_VERSION' and parts[0] == 'minos') or
                        (command.startswith('LC_VERSION_MIN_') and parts[0] == 'version')):
                        minimums.append(tuple(int(v) for v in parts[1].split('.')))
                limit = (13, 0) if key[0] == 'macos' else (16, 0)
                assert minimums and all(v[:2] <= limit for v in minimums), f'{key}: native deployment target exceeds {limit}: {max(minimums, default=())}'
                expected = target_dir / targets[key] / 'release/libbark_bolt_ffi.a'
                assert checksum(thin) == checksum(expected), f'{key} is not the current release build'
                seen.add(key)
        assert seen == set(targets), f'Missing Apple slices: {set(targets) - seen}'
    print(f'All {len(targets)} release Apple ABI slices match Cargo output and declared deployment targets')
    return framework


def package_version():
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--format-version=1', '--no-deps', '--locked'], cwd=ROOT, text=True))
    return next(p['version'] for p in metadata['packages']
                if Path(p['manifest_path']).resolve() == (ROOT / 'Cargo.toml').resolve())


def require_unpublished_version(version):
    tag = 'refs/tags/v' + version
    local = subprocess.run(['git', 'show-ref', '--verify', '--quiet', tag], cwd=ROOT)
    if local.returncode == 0:
        raise RuntimeError(f'Release tag v{version} already exists; bump the Cargo version first')
    if local.returncode != 1:
        raise RuntimeError('Cannot check local release tags')
    remote = subprocess.run(['git', 'ls-remote', '--exit-code', '--tags', 'origin', tag],
                            cwd=ROOT, capture_output=True)
    if remote.returncode == 0:
        raise RuntimeError(f'Release tag v{version} already exists; bump the Cargo version first')
    if remote.returncode != 2:
        raise RuntimeError('Cannot check remote release tags; refusing to stage')


def stage():
    version = package_version()
    require_unpublished_version(version)
    framework = check_build()
    generated = framework.parent
    SDK.mkdir(parents=True, exist_ok=True)
    for name in ['FFI', 'Sources']:
        destination = SDK / name
        if destination.exists():
            shutil.rmtree(destination)
        shutil.copytree(generated / name, destination)
    archive = ROOT / 'dist/BarkBoltFFI.xcframework.zip'
    with zipfile.ZipFile(archive, 'w') as output:
        for file in sorted(framework.rglob('*')):
            if file.is_file():
                info = zipfile.ZipInfo(file.relative_to(framework.parent).as_posix(), (1980, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = 0o100644 << 16
                with file.open('rb') as source, output.open(info, 'w') as destination:
                    shutil.copyfileobj(source, destination)
    archive_hash = checksum(archive)
    package = (generated / 'Package.swift').read_text().replace('path: "', 'path: "sdk/swift/')
    package = package.replace('path: "sdk/swift/BarkBoltFFI.xcframework"',
        'url: "https://github.com/smolcars/bark-bolt-ffi/releases/download/v' + version + '/BarkBoltFFI.xcframework.zip", checksum: "' + archive_hash + '"')
    (ROOT / 'Package.swift').write_text(package + '\n')
    (SDK / 'provenance.json').write_text(json.dumps({
        'version': version, 'bark': '0.8.0', 'bark_revision': '959a489ff4934b181783b41c7d3ee68d85b95cbf', 'boltffi': '0.31.1',
        'rust': '1.99.1', 'minimum_ios': '16.0', 'minimum_macos': '13.0', 'profile': 'release', 'archive_sha256': archive_hash, 'source_sha256': source_digest(),
        'files': artifacts(), 'package_sha256': checksum(ROOT / 'Package.swift'),
    }, indent=2, sort_keys=True) + '\n')
    print(f'Staged release SwiftPM manifest and generated sources; archive: {archive.stat().st_size} bytes')


def verify(check_source=False):
    metadata = json.loads((SDK / 'provenance.json').read_text())
    if check_source:
        assert metadata['source_sha256'] == source_digest(), 'Swift SDK is stale; rebuild release SDK and stage it'
        assert metadata['version'] == package_version(), 'Swift SDK version differs from Cargo'
    assert metadata['files'] == artifacts(), 'Swift SDK files changed or are missing'
    assert metadata['package_sha256'] == checksum(ROOT / 'Package.swift'), 'Root SwiftPM manifest changed'
    assert metadata['profile'] == 'release'
    assert metadata['archive_sha256'] in (ROOT / 'Package.swift').read_text()
    print('Committed Swift release files and manifest checksums match')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_mutually_exclusive_group()
    commands.add_argument('--stage-release', action='store_true')
    commands.add_argument('--check-build', action='store_true')
    commands.add_argument('--check-source', action='store_true', help='Require release artifacts to match current source/version')
    options = parser.parse_args()
    if options.check_build:
        check_build(pr_build=os.environ.get('BARK_PR_BUILD') == '1')
    else:
        if options.stage_release:
            stage()
        verify(check_source=options.stage_release or options.check_source)
