"""Check full inventory coverage and rejection of ambiguous dispatcher evidence."""
import csv
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest


def load_script(name):
    path = Path(__file__).with_name(name)
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class PathAuditTests(unittest.TestCase):
    def test_scalar_wait_route_names_its_handler_without_implying_coverage(self):
        module = load_script('audit-syscall-paths.py')
        report = module.inventory(Path(__file__).resolve().parents[1])
        row = next(row for row in report['syscalls'] if row['number'] == 455)
        self.assertEqual(row['status'], 'dispatched')
        self.assertIn('futex_scalar::wait', row['handlers'])
        self.assertEqual(row['completion'], 'unproven')
        self.assertTrue(all(path['status'] == 'pending' for path in row['paths'].values()))

    def test_every_linux_table_entry_has_all_review_axes_without_claims(self):
        module = load_script('audit-syscall-paths.py')
        root = Path(__file__).resolve().parents[1]
        report = module.inventory(root)
        with (root / 'docs/architecture-v2-syscalls.csv').open(encoding='utf-8-sig', newline='') as stream:
            expected = {(int(row['number']), row['name']) for row in csv.DictReader(stream)}
        actual = {(row['number'], row['name']) for row in report['syscalls']}
        self.assertEqual(actual, expected)
        self.assertEqual(len(actual), len(report['syscalls']))
        self.assertEqual(report['counts']['required_path_reviews'], len(expected) * len(module.AXES))
        self.assertEqual(report['counts']['verified_path_reviews'], 0)
        for row in report['syscalls']:
            self.assertEqual(set(row['paths']), set(module.AXES))
            self.assertEqual(row['completion'], 'unproven')
            for path in row['paths'].values():
                self.assertEqual(path['status'], 'pending')
                self.assertEqual(path['evidence'], [])
        exported = list(csv.DictReader(io.StringIO(module.csv_text(report))))
        self.assertEqual({(int(row['number']), row['name']) for row in exported}, expected)

    def test_duplicate_route_is_an_error_instead_of_overwriting_evidence(self):
        module = load_script('audit-syscalls.py')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'libs/libc/src').mkdir(parents=True)
            (root / 'docs').mkdir()
            (root / 'docs/architecture-v2-syscalls.csv').write_text('number,name\n449,futex_waitv\n', encoding='utf-8')
            (root / 'libs/libc/src/sysadmin.rs').write_text('''
fn dispatch_syscall() {
    let res = match number {
        449 => first(),
        449 => second(),
        _ => -i64::from(ENOSYS),
    };
}
''', encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'duplicate syscall dispatch 449'):
                module.audit(root)


if __name__ == '__main__':
    unittest.main()
