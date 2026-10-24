"""Stage the real v0.7.0 Apple CI build after verifying its source and archive.

Run from the repository root after downloading artifact 12575299367 to
/tmp/bark-v070-dl/apple-ci.zip. This reuses the successful hosted five-ABI build;
it does not claim a second local build or a local Cargo archive comparison.
"""
import importlib.util
import json
from pathlib import Path
import plistlib
import subprocess
import shutil
import tempfile
import zipfile

root = Path(__file__).resolve().parents[2]
run_id = '40132677901'
source = '6961e82508a52d297a4c72d65131e0f5f8871bdc'
archive = Path('/tmp/bark-v070-dl/apple-ci.zip')
expected = 'c095818a21d1cd0ce8056a089295690303a9c65cf7f22c4da8313157c33df0e8'
spec = importlib.util.spec_from_file_location('artifact', root / 'scripts/swift-artifact.py')
artifact = importlib.util.module_from_spec(spec)
spec.loader.exec_module(artifact)
assert artifact.checksum(archive) == expected, 'Downloaded Actions artifact digest differs'
run = json.loads(subprocess.check_output(['/opt/homebrew/bin/gh', 'run', 'view', run_id,
                                         '--json', 'headSha,jobs'], cwd=root))
assert run['headSha'] == source
apple = next(job for job in run['jobs'] if job['name'] == 'apple')
# Staging may overlap cache saving; publication separately requires a green full run.
assert all(step['conclusion'] == 'success' for step in apple['steps']
           if not step['name'].startswith('Post ')), apple
for name in ['Run python3 scripts/swift-artifact.py --check-build',
             'Run swift test --package-path tests/swift', 'Test iOS app']:
    assert any(step['name'] == name and step['conclusion'] == 'success' for step in apple['steps']), name
subprocess.run(['git', 'diff', '--exit-code', source, '--', 'src', 'Cargo.toml', 'Cargo.lock',
                'boltffi.toml', 'rust-toolchain.toml', '.cargo/config.toml',
                'scripts/build-sdk.sh', 'sdk/swift-support'], cwd=root, check=True)

with tempfile.TemporaryDirectory(prefix='bark-v070-ci-') as directory:
    extracted = Path(directory)
    with zipfile.ZipFile(archive) as package:
        for name in package.namelist():
            path = Path(name)
            if name.startswith('dist/apple/') and '..' not in path.parts and not name.endswith('/'):
                destination = extracted / path
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(package.read(name))
    framework = extracted / 'dist/apple/BarkBoltFFI.xcframework'
    info = plistlib.loads((framework / 'Info.plist').read_bytes())
    seen = set()
    for library in info['AvailableLibraries']:
        binary = framework / library['LibraryIdentifier'] / library['LibraryPath']
        output = subprocess.check_output(['xcrun', 'otool', '-arch', 'all', '-l', str(binary)], text=True)
        command = ''
        minimums = []
        for line in output.splitlines():
            parts = line.split()
            if len(parts) == 2 and parts[0] == 'cmd':
                command = parts[1]
            if len(parts) == 2 and ((command == 'LC_BUILD_VERSION' and parts[0] == 'minos') or
                (command.startswith('LC_VERSION_MIN_') and parts[0] == 'version')):
                minimums.append(tuple(int(v) for v in parts[1].split('.')))
        limit = (13, 0) if library['SupportedPlatform'] == 'macos' else (16, 0)
        assert minimums and all(value[:2] <= limit for value in minimums)
        for arch in library['SupportedArchitectures']:
            seen.add((library['SupportedPlatform'], library.get('SupportedPlatformVariant'), arch))
    assert seen == set(artifact.TARGETS)
    local = root / 'dist/apple'
    if local.exists():
        shutil.rmtree(local)
    shutil.copytree(framework.parent, local)
    framework = local / 'BarkBoltFFI.xcframework'
    # The hosted check-build step compared all five archives with that run's Cargo outputs.
    # The authenticated artifact digest above binds those exact files to the successful run.
    (root / 'dist').mkdir(exist_ok=True)
    artifact.check_build = lambda: framework
    artifact.stage()
    artifact.verify(check_source=True)
print(json.dumps({'run_id': run_id, 'source': source, 'actions_artifact_sha256': expected,
                  'xcframework_sha256': artifact.checksum(root / 'dist/BarkBoltFFI.xcframework.zip')}))
