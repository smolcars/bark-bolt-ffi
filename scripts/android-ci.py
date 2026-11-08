#!/usr/bin/env python3
"""Transfer compiled Android acceptance tests between the build and emulator jobs."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]
RUNTIME = Path('artifacts/android-runtime')
MANIFEST = RUNTIME / 'manifest.json'
ARCHIVE = Path('artifacts/android-build.tar.gz')
PAYLOAD_ROOTS = (
    'sdk/android/build/outputs/aar/', 'sdk/android/build/publications/release/',
    'sdk/jvm/build/libs/', 'sdk/jvm/build/publications/jvm/',
    'sdk/jvm/build/ci-runtime/', 'sdk/jvm/build/reports/tests/',
    'examples/android/build/outputs/apk/', 'examples/android/build/reports/',
    'artifacts/android-runtime/',
)


def payload_path(name):
    path = PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts or not name.startswith(PAYLOAD_ROOTS):
        raise ValueError(f'Unexpected runtime payload path: {name}')
    return path


def checksum(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for part in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(part)
    return digest.hexdigest()


def source_commit():
    return subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()


def verify(manifest, commit, root):
    if manifest['source_commit'] != commit:
        raise ValueError('Android runtime artifacts were built from a different commit')
    if not manifest['files'] or not manifest['rust_tests']:
        raise ValueError('Android runtime payload is incomplete')
    for name, digest in manifest['files'].items():
        path = root / payload_path(name)
        if not path.resolve().is_relative_to(root.resolve()) or path.is_symlink():
            raise ValueError(f'Runtime payload escapes checkout: {name}')
        if checksum(path) != digest:
            raise ValueError(f'Runtime payload checksum mismatch: {name}')
    for name in manifest['rust_tests']:
        if name not in manifest['files'] or not name.startswith(str(RUNTIME / 'rust') + '/'):
            raise ValueError(f'Unverified Rust test executable: {name}')


def portable_native(path):
    result = subprocess.run(['/usr/bin/ldd', str(path)], text=True, capture_output=True, check=True)
    if 'not found' in result.stdout or '/nix/store/' in result.stdout:
        raise ValueError(f'{path} cannot run outside its build environment:\n{result.stdout}')


def bundle():
    subprocess.run(['git', 'diff', '--exit-code', 'HEAD'], cwd=ROOT, check=True)
    runtime = ROOT / RUNTIME
    if runtime.exists():
        shutil.rmtree(runtime)
    (runtime / 'rust').mkdir(parents=True)
    tests = []
    for line in (ROOT / 'target/ci-rust-tests.jsonl').read_text().splitlines():
        message = json.loads(line)
        if message.get('reason') == 'compiler-artifact' and message.get('profile', {}).get('test') and message.get('executable'):
            executable = Path(message['executable'])
            destination = runtime / 'rust' / executable.name
            portable_native(executable)
            shutil.copy2(executable, destination)
            tests.append(destination.relative_to(ROOT).as_posix())
    if not tests:
        raise ValueError('Cargo produced no test executables')
    natives = list((ROOT / 'dist/android/desktopJniLibs').rglob('*.so'))
    if not natives:
        raise ValueError('No desktop JNI shared library was built')
    for native in natives:
        portable_native(native)
    files = set()
    required = [
        'sdk/android/build/outputs/aar/*.aar', 'sdk/android/build/publications/release/*.xml',
        'sdk/jvm/build/libs/*.jar', 'sdk/jvm/build/publications/jvm/*.xml',
        'sdk/jvm/build/ci-runtime', 'examples/android/build/outputs/apk', str(RUNTIME),
        'examples/android/build/outputs/apk/debug/android-debug.apk',
        'examples/android/build/outputs/apk/androidTest/debug/android-debug-androidTest.apk',
        'examples/android/build/outputs/apk/release/*.apk',
    ]
    optional = ['sdk/jvm/build/reports/tests', 'examples/android/build/reports']
    for pattern in required + optional:
        matches = list(ROOT.glob(pattern))
        if not matches and pattern in required:
            raise ValueError(f'Missing Android build output: {pattern}')
        for match in matches:
            if match.is_dir():
                files.update(path for path in match.rglob('*') if path.is_file())
            else:
                files.add(match)
    manifest = {'source_commit': source_commit(), 'rust_tests': sorted(set(tests)),
                'files': {path.relative_to(ROOT).as_posix(): checksum(path) for path in sorted(files)}}
    verify(manifest, source_commit(), ROOT)
    (ROOT / MANIFEST).write_text(json.dumps(manifest, indent=2) + '\n')
    with tarfile.open(ROOT / ARCHIVE, 'w:gz') as archive:
        for name in sorted([*manifest['files'], MANIFEST.as_posix()]):
            archive.add(ROOT / name, arcname=name, recursive=False)
    print(f'{ARCHIVE}: {len(files)} files, commit {manifest["source_commit"]}')


def unpack():
    with tarfile.open(ROOT / ARCHIVE) as archive:
        members = archive.getmembers()
        names = [member.name for member in members]
        for member in members:
            payload_path(member.name)
            if not member.isfile():
                raise ValueError(f'Runtime archive contains a non-regular file: {member.name}')
        if len(names) != len(set(names)):
            raise ValueError('Runtime archive contains duplicate paths')
        manifest = json.load(archive.extractfile(MANIFEST.as_posix()))
        if manifest['source_commit'] != source_commit():
            raise ValueError('Android runtime artifacts were built from a different commit')
        if set(names) != set(manifest['files']) | {MANIFEST.as_posix()}:
            raise ValueError('Runtime archive differs from its file manifest')
        archive.extractall(ROOT, filter='data')
    verify(manifest, source_commit(), ROOT)
    print(f'Verified Android runtime payload for {manifest["source_commit"]}')


def rust_tests(manifest):
    for name in manifest['rust_tests']:
        subprocess.run([str(ROOT / name), '--test-threads=1'], cwd=ROOT, check=True)


def jvm_tests():
    runtime = ROOT / 'sdk/jvm/build/ci-runtime'
    tests = (runtime / 'test-classes.txt').read_text().splitlines()
    if not tests:
        raise ValueError('No JVM contract classes were staged')
    jars = sorted((ROOT / 'sdk/jvm/build/libs').glob('*.jar'))
    if len(jars) != 1:
        raise ValueError(f'Expected exactly one packaged JVM SDK: {jars}')
    for main in [runtime / 'main', jars[0]]:
        classpath = os.pathsep.join(map(str, [runtime / 'test', runtime / 'lib/*', main]))
        subprocess.run(['java', '-Dbark.fixtures=' + str(ROOT / 'tests/fixtures'), '-cp', classpath,
                        'org.junit.runner.JUnitCore', *tests], cwd=ROOT, check=True)


def instrumentation_count(output):
    match = re.search(r'^OK \(([1-9][0-9]*) tests?\)\s*$', output, re.MULTILINE)
    if not match or any(error in output for error in ['FAILURES!!!', 'INSTRUMENTATION_FAILED', 'shortMsg=']):
        raise ValueError('Android instrumentation did not report successful tests')
    return int(match.group(1))


def instrument():
    app = ROOT / 'examples/android/build/outputs/apk'
    for apk in [app / 'debug/android-debug.apk', app / 'androidTest/debug/android-debug-androidTest.apk']:
        subprocess.run(['adb', 'install', '-r', str(apk)], check=True)
    result = subprocess.run(['adb', 'shell', 'am', 'instrument', '-w', '-r',
        'io.github.niteshbalusu11.bark.example.test/androidx.test.runner.AndroidJUnitRunner'],
        text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=True)
    report = ROOT / 'examples/android/build/reports/instrumentation.txt'
    report.parent.mkdir(parents=True, exist_ok=True)
    report.write_text(result.stdout)
    print(result.stdout, flush=True)
    print(f'{instrumentation_count(result.stdout)} Android instrumentation tests passed')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['bundle', 'unpack', 'rust', 'jvm', 'instrument'])
    command = parser.parse_args().command
    if command == 'bundle':
        bundle()
    elif command == 'unpack':
        unpack()
    else:
        manifest = json.loads((ROOT / MANIFEST).read_text())
        verify(manifest, source_commit(), ROOT)
        if command == 'rust':
            rust_tests(manifest)
        elif command == 'jvm':
            jvm_tests()
        else:
            instrument()
