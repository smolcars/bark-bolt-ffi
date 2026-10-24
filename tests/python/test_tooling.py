import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def load_script(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


artifact = load_script('swift-artifact')
runner = load_script('ci-regtest-runner')
alignment = load_script('check-android-alignment')


class AndroidAlignmentTests(unittest.TestCase):
    def test_all_load_segments_must_be_aligned(self):
        good = '  LOAD 0x0 0x0 0x0 0x100 0x100 R E 0x4000\n'
        self.assertEqual([0x4000, 0x10000], alignment.load_alignments(
            good + good.replace('0x4000', '0x10000')))
        with self.assertRaisesRegex(ValueError, 'below 0x4000'):
            alignment.load_alignments(good + good.replace('0x4000', '0x1000'))

    def test_missing_or_malformed_load_segments_fail(self):
        for output in ['', '  NOTE 0x4000', '  LOAD malformed']:
            with self.subTest(output=output), self.assertRaises(ValueError):
                alignment.load_alignments(output)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix='bark-tooling-test-')
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name) / 'repo'
        self.root.mkdir()
        self.sdk = self.root / 'sdk/swift'
        changes = patch.multiple(artifact, ROOT=self.root, SDK=self.sdk)
        changes.start()
        self.addCleanup(changes.stop)
        files = {
            'src/lib.rs': '// fixture\n',
            'Cargo.toml': '[package]\nname="fixture"\nversion="9.8.7"\nedition="2021"\n',
            'Cargo.lock': 'version = 4\n[[package]]\nname="fixture"\nversion="9.8.7"\n',
            'rust-toolchain.toml': '[toolchain]\nchannel="1.99.1"\n',
            'boltffi.toml': '', '.cargo/config.toml': '', 'sdk/swift-support/Concurrency.swift': '',
            'scripts/build-sdk.sh': '', 'sdk/swift/Sources/fixture.swift': '// SDK\n',
            'Package.swift': '// immutable checksum abc123\n',
        }
        for name, content in files.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        self.write_provenance()
        origin = Path(directory.name) / 'origin.git'
        subprocess.run(['git', 'init', '--bare', '-q', str(origin)], check=True)
        self.git('init', '-q')
        self.git('add', 'Cargo.toml')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                 'commit', '-qm', 'fixture')
        self.git('remote', 'add', 'origin', str(origin))

    def git(self, *args):
        return subprocess.run(['git', *args], cwd=self.root, check=True, capture_output=True)

    def write_provenance(self):
        (self.sdk / 'provenance.json').write_text(json.dumps({
            'version': '9.8.7', 'profile': 'release', 'archive_sha256': 'abc123',
            'source_sha256': artifact.source_digest(), 'files': artifact.artifacts(),
            'package_sha256': artifact.checksum(self.root / 'Package.swift'),
        }))

    def test_dev_edit_keeps_published_integrity_but_release_check_rejects_it(self):
        (self.root / 'src/lib.rs').write_text('// a development-only edit\n')
        artifact.verify()
        with self.assertRaisesRegex(AssertionError, 'stale'):
            artifact.verify(check_source=True)

    def test_committed_sdk_and_manifest_tampering_are_still_rejected(self):
        binding = self.sdk / 'Sources/fixture.swift'
        binding.write_text('// tampered\n')
        with self.assertRaisesRegex(AssertionError, 'SDK files'):
            artifact.verify()
        binding.write_text('// SDK\n')
        (self.root / 'Package.swift').write_text('// tampered manifest\n')
        with self.assertRaisesRegex(AssertionError, 'manifest'):
            artifact.verify()

    def test_remote_release_tag_rejects_stage_before_build_or_writes(self):
        self.git('tag', 'v9.8.7')
        self.git('push', '-q', 'origin', 'refs/tags/v9.8.7')
        self.git('tag', '-d', 'v9.8.7')
        before = (self.root / 'Package.swift').read_bytes()
        with patch.object(artifact, 'check_build', side_effect=AssertionError('build called before tag guard')):
            with self.assertRaisesRegex(RuntimeError, 'already exists'):
                artifact.stage()
        self.assertEqual(before, (self.root / 'Package.swift').read_bytes())

    def test_unreachable_remote_refuses_stage_before_build(self):
        self.git('remote', 'set-url', 'origin', str(self.root / 'missing-origin'))
        with patch.object(artifact, 'check_build', side_effect=AssertionError('build called before remote check')):
            with self.assertRaisesRegex(RuntimeError, 'Cannot check remote release tags'):
                artifact.stage()

    def test_staging_uses_cargo_version_and_archive_ignores_mtime(self):
        generated = self.root / 'dist/apple'
        framework = generated / 'BarkBoltFFI.xcframework'
        framework.mkdir(parents=True)
        binary = framework / 'fixture.a'
        binary.write_bytes(b'unchanged native archive fixture')
        for name in ['FFI', 'Sources']:
            (generated / name).mkdir()
            (generated / name / 'fixture').write_text(name)
        (generated / 'Package.swift').write_text('path: "BarkBoltFFI.xcframework"')
        with patch.object(artifact, 'check_build', return_value=framework):
            os.utime(binary, (1577836800, 1577836800))
            artifact.stage()
            archive = self.root / 'dist/BarkBoltFFI.xcframework.zip'
            first = hashlib.sha256(archive.read_bytes()).hexdigest()
            os.utime(binary, (1735689600, 1735689600))
            artifact.stage()
            self.assertEqual(first, hashlib.sha256(archive.read_bytes()).hexdigest())
        self.assertEqual('9.8.7', json.loads((self.sdk / 'provenance.json').read_text())['version'])
        self.assertIn('/v9.8.7/', (self.root / 'Package.swift').read_text())


class RunnerTests(unittest.TestCase):
    def test_child_does_not_inherit_authentication_environment(self):
        parent = {'HOME': '/tmp/fixture-user', 'PATH': '/usr/bin', 'TMPDIR': '/tmp',
                  'GH_TOKEN': 'fixture-secret', 'GITHUB_TOKEN': 'fixture-secret',
                  'SSH_AUTH_SOCK': '/tmp/fixture-agent', 'AWS_SECRET_ACCESS_KEY': 'fixture-secret'}
        child = runner.child_environment('ios-id', 'android-id', parent)
        self.assertFalse({'GH_TOKEN', 'GITHUB_TOKEN', 'SSH_AUTH_SOCK', 'AWS_SECRET_ACCESS_KEY'} & child.keys())
        self.assertEqual('/tmp/fixture-user', child['HOME'])
        self.assertEqual('ios-id', child['IOS_DEVICE'])

    def test_fork_approval_policy_fails_closed(self):
        for policy in ['first_time_contributors', 'first_time_contributors_new_to_github', None]:
            with self.assertRaisesRegex(RuntimeError, 'all_external_contributors'):
                runner.require_fork_approval({'approval_policy': policy})
        runner.require_fork_approval({'approval_policy': 'all_external_contributors'})

    def test_dispatch_is_fixed_to_trusted_workflow_and_main(self):
        with patch.object(runner.subprocess, 'run') as invoke:
            runner.dispatch('/fixture/gh', 'bark-bolt-regtest-fixture', {'PATH': '/usr/bin'})
        args = invoke.call_args.args[0]
        self.assertEqual(['/fixture/gh', 'workflow', 'run', 'integration.yml', '--repo',
                          runner.REPOSITORY, '--ref', 'main', '--field',
                          'runner_label=bark-bolt-regtest-fixture'], args)
        self.assertTrue(invoke.call_args.kwargs['check'])


if __name__ == '__main__':
    unittest.main()
