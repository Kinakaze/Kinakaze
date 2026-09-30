"""Format/integrity regression tests for the streaming I/O trace reader."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('io_trace_analysis', Path(__file__).with_name('analyze-io-trace.py'))
trace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace)


def event(seq, parent, elapsed, op, result=0, args=(0, 0, 0), path=''):
    encoded_op, encoded_path = op.encode(), path.encode()
    return trace.HEADER.pack(trace.HEADER.size + len(encoded_op) + len(encoded_path),
                             len(encoded_op), 0, seq, parent, 1, elapsed, *args,
                             result, 42, 3, len(encoded_path), 1) + encoded_op + encoded_path


class TraceTests(unittest.TestCase):
    def test_nested_timings_pending_errno_utf8_and_checkpoint_loss(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'session/io-trace'
            source.mkdir(parents=True)
            (source / 'io-42-3-1.ktrace').write_bytes(
                event(2, 1, 30, 'native.NtCreateFile', 0x103) +
                event(3, 1, 20, 'trace.flush') +
                event(1, 0, 100, 'vfs.open', -2, path='/路径/a\tb') +
                event(4, 0, 0, 'trace.checkpoint', args=(7, 0, 0)))
            report = trace.analyze(root, root / 'analysis')
            rows = {row['op']: row for row in report['operations']}
            self.assertEqual(rows['vfs.open']['exclusive_ms'], 50 / 1e6)
            self.assertEqual(rows['vfs.open']['results'], {'-2': 1})
            self.assertEqual(rows['native.NtCreateFile']['results'], {'pending': 1})
            self.assertEqual(report['integrity']['dropped'], 7)
            self.assertEqual(report['integrity']['unresolved_parent_spans'], 0)
            self.assertEqual(report['top_paths'][0]['path'], '/路径/a\tb')

    def test_truncated_tail_is_reported_and_prior_records_remain(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'session/io-trace'
            source.mkdir(parents=True)
            (source / 'io-broken.ktrace').write_bytes(event(1, 0, 100, 'vfs.read', 16) + b'bad')
            report = trace.analyze(root, root / 'analysis')
            self.assertEqual(report['integrity']['records'], 1)
            self.assertEqual(len(report['integrity']['parse_errors']), 1)
            self.assertEqual(report['integrity']['files_without_checkpoint'], 1)
            self.assertEqual(report['operations'][0]['bytes'], 16)


if __name__ == '__main__':
    unittest.main()
