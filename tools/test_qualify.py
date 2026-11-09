"""Run with: python -B -m unittest discover -s tools -p 'test_*.py'."""
import argparse
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import qualify


class QualificationTests(unittest.TestCase):
    def args(self, root):
        return argparse.Namespace(source=str(root/'source'), workspace=str(root/'workspace'),
                                  report=str(root/'report'), subtree=None, budget_mib=1,
                                  max_file_mib=1, max_files=20, scan_limit=100)

    def test_selection_is_bounded_deterministic_and_has_no_duplicates(self):
        candidates = {f'f{i}': size for i, size in enumerate([0, 12, 1024, 80000, 90000, 10*qualify.MIB])}
        selected = qualify.select(candidates, qualify.MIB, 4)
        self.assertEqual(selected, qualify.select(dict(reversed(list(candidates.items()))), qualify.MIB, 4))
        self.assertLessEqual(len(selected), 4)
        self.assertLessEqual(sum(size for _, size in selected), qualify.MIB)
        self.assertEqual(len(selected), len(set(name for name, _ in selected)))

    def test_prepare_produces_independent_copies_and_preserves_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root/'source'; source.mkdir()
            (source/'photo').write_bytes(b'photo data')
            (source/'.keeper').mkdir()
            (source/'.keeper/private').write_bytes(b'not sample data')
            (source/'.keeperignore').write_bytes(b'policy')
            args = self.args(root)
            qualify.prepare(args)
            workspace, report, manifest = qualify.load_workspace(args.workspace)
            self.assertEqual(manifest['files_per_sample'], 1)
            self.assertEqual((source/'photo').read_bytes(), b'photo data')
            inodes = {source.joinpath('photo').stat().st_ino}
            for name in ['sample-a', 'sample-b']:
                path = workspace/name/'photo'
                self.assertEqual(path.read_bytes(), b'photo data')
                inodes.add(path.stat().st_ino)
                self.assertFalse((workspace/name/'.keeper').exists())
            self.assertEqual(len(inodes), 3)
            self.assertTrue((report/'preparation.json').is_file())

    def test_refuses_existing_destinations_and_report_inside_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); (root/'source').mkdir()
            args = self.args(root)
            args.report = str(root/'source/report')
            with self.assertRaises(ValueError):
                qualify.prepare(args)
            self.assertFalse((root/'workspace').exists())
            args = self.args(root)
            (root/'workspace').mkdir()
            with self.assertRaises(ValueError):
                qualify.prepare(args)

    def test_workspace_replacement_and_symlink_members_fail_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); (root/'source').mkdir(); (root/'source/file').write_bytes(b'x')
            args = self.args(root); qualify.prepare(args)
            workspace = root/'workspace'
            (workspace/'sample-a').rename(workspace/'saved')
            (workspace/'sample-a').symlink_to(root/'source', target_is_directory=True)
            with self.assertRaises(ValueError):
                qualify.load_workspace(workspace)
            self.assertEqual((root/'source/file').read_bytes(), b'x')

    def test_copy_verification_will_notice_equal_size_mutation(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)/'file'; path.write_bytes(b'original')
            before = qualify.digest(path)
            meta = path.stat(); path.write_bytes(b'modified')
            os.utime(path, ns=(meta.st_atime_ns,meta.st_mtime_ns))
            self.assertNotEqual(qualify.digest(path), before)

    def test_probe_records_observations_without_assuming_resolution(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = qualify.timestamp_probe(Path(tmp)/'probe', trials=2)
            self.assertEqual(len(result['observations']), 9)
            for observation in result['observations']:
                self.assertLessEqual(observation['unchanged_fingerprint'], 2)
            json.dumps(result)

    def test_runner_records_failures_without_calling_them_success(self):
        with tempfile.TemporaryDirectory() as tmp:
            runner = qualify.Runner(Path(tmp), Path('/bin/false'), 10)
            runner.command('failed', ['/bin/false'])
            self.assertFalse(runner.results[0]['passed'])
            self.assertEqual(runner.results[0]['exit_code'], 1)
            self.assertTrue((Path(tmp)/'commands.json').is_file())

    def test_failed_build_cannot_fall_back_to_an_old_binary(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); (root/'source').mkdir(); (root/'source/file').write_bytes(b'x')
            args = self.args(root); qualify.prepare(args)
            calls = []
            def fake_command(runner, label, argv, **kwargs):
                calls.append(label)
                runner.results.append({'passed':label != 'release-build'})
                return ''
            run_args = argparse.Namespace(workspace=args.workspace, binary=str(root/'not-built'),
                                          timeout=10, skip_suite=True, evict_sample_cache=False)
            with mock.patch.object(qualify.Runner, 'command', fake_command):
                with self.assertRaisesRegex(ValueError, 'stale binary'):
                    qualify.run(run_args)
            self.assertEqual(calls, ['environment', 'release-build'])
            self.assertFalse((root/'workspace/sample-a/.keeper').exists())

    def test_timeout_kills_owned_command_and_retains_logs(self):
        with tempfile.TemporaryDirectory() as tmp:
            runner = qualify.Runner(Path(tmp), Path('/bin/false'), .02)
            with self.assertRaises(TimeoutError):
                runner.command('timeout', ['/bin/sleep', '60'])
            self.assertTrue((Path(tmp)/'timeout.stdout').exists())
            self.assertTrue((Path(tmp)/'timeout.stderr').exists())

    def test_positive_arguments(self):
        for value in ['0','-1']:
            with self.assertRaises(argparse.ArgumentTypeError):
                qualify.positive(value)
        self.assertEqual(qualify.positive('10'),10)


if __name__ == '__main__':
    unittest.main()
