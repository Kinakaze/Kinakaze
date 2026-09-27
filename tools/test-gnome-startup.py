"""Check desktop configuration and timing without launching any guest or GUI."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from gnome_ibus import (LIBRARY, RESOURCE, MANIFEST, QUEUE_SPAWN,
                        prepare_ibus_override, ibus_resource_overlay)


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


launcher = load('run-gnome')
prepare = load('prepare-desktop')


class StartupTests(unittest.TestCase):
    def test_split_lines_and_duplicate_markers(self):
        timings = launcher.StartupTimings()
        timings.feed('stderr', b'GNOME Shell-Message: GNOME Shell started ', 1)
        self.assertNotIn('shell-ready', timings.stages)
        timings.feed('stderr', b'at today\n', 2)
        timings.feed('stderr', b'GNOME Shell started at today\n', 3)
        self.assertEqual(timings.stages['shell-ready'], 2)

    def test_running_and_application_stdout_are_not_readiness(self):
        timings = launcher.StartupTimings()
        timings.feed('stderr', b'Running GNOME Shell (using mutter 43)\n', .5)
        timings.feed('stdout', b'GNOME Shell started at today\nGNOME startup: shell-ready\n', 1)
        self.assertEqual(timings.stages, {'mutter-running': .5})

    def test_observation_deadlines_and_early_exit(self):
        class Process:
            def __init__(self, code):
                self.code = code

            def poll(self):
                return self.code

        with tempfile.TemporaryDirectory() as directory:
            out, err = Path(directory) / 'stdout', Path(directory) / 'stderr'
            out.write_bytes(b'IBus ready: unix:path=/example\n')
            err.write_bytes(b'')
            with patch.object(launcher.time, 'monotonic', return_value=2):
                timings = launcher.StartupTimings()
                self.assertTrue(launcher.observe_startup(Process(None), out, err, 0, 1, None, timings))
                self.assertNotIn('shell-ready', timings.stages)
                self.assertFalse(launcher.observe_startup(Process(1), out, err, 0, 1, None, timings))
                err.write_bytes(b'GNOME Shell started at today\n')
                self.assertTrue(launcher.observe_startup(Process(None), out, err, 0, 30, 0, timings))
                self.assertEqual(timings.stages['shell-ready'], 2)

    def test_embedded_guest_programs_compile(self):
        compile(launcher.SESSION, '<session>', 'exec')
        compile(launcher.GUEST, '<guest>', 'exec')


class IBusOverrideTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        library = self.root / LIBRARY
        library.parent.mkdir(parents=True)
        library.write_bytes(b'installed-shell-version-one')
        self.source = (QUEUE_SPAWN + '\n// independent restart logic\n'
                       'if (!isSystemdService) this._spawn(["-r"]);\n').encode()

    def test_override_is_bound_to_elf_and_resource_contents(self):
        self.assertTrue(prepare_ibus_override(self.root, self.source))
        self.assertIsNotNone(ibus_resource_overlay(self.root))
        (self.root / LIBRARY).write_bytes(b'updated-shell-version-two')
        self.assertIsNone(ibus_resource_overlay(self.root))
        self.assertTrue(prepare_ibus_override(self.root, self.source))
        self.assertIsNotNone(ibus_resource_overlay(self.root))
        (self.root / RESOURCE).write_bytes(b'changed-resource')
        self.assertIsNone(ibus_resource_overlay(self.root))

    def test_missing_unknown_or_corrupt_override_falls_back(self):
        self.assertIsNone(ibus_resource_overlay(self.root))
        self.assertFalse(prepare_ibus_override(self.root, b'new upstream layout'))
        self.assertFalse((self.root / MANIFEST).exists())
        self.assertTrue(prepare_ibus_override(self.root, self.source))
        (self.root / MANIFEST).write_bytes(b'{broken')
        self.assertIsNone(ibus_resource_overlay(self.root))

    def test_restart_code_is_preserved_and_preparation_is_idempotent(self):
        self.assertTrue(prepare_ibus_override(self.root, self.source))
        original = (self.root / RESOURCE).read_bytes()
        self.assertIn(b'if (!isSystemdService) this._spawn(["-r"]);', original)
        self.assertTrue(prepare_ibus_override(self.root, self.source))
        self.assertEqual((self.root / RESOURCE).read_bytes(), original)


class ConfigurationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / 'etc').mkdir()
        for zone in ('Etc/UTC', 'Asia/Shanghai'):
            path = self.root / 'usr/share/zoneinfo' / zone
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(('TZif-' + zone).encode())

    def test_fresh_root_has_explicit_utc_and_is_idempotent(self):
        prepare.prepare_session_defaults(self.root)
        prepare.prepare_session_defaults(self.root)
        self.assertEqual((self.root / 'etc/timezone').read_bytes(), b'Etc/UTC\n')
        self.assertEqual((self.root / 'etc/localtime').read_bytes(), b'TZif-Etc/UTC')

    def test_existing_named_zone_repairs_missing_localtime(self):
        (self.root / 'etc/timezone').write_bytes(b'Asia/Shanghai\n')
        prepare.prepare_session_defaults(self.root)
        self.assertEqual((self.root / 'etc/localtime').read_bytes(), b'TZif-Asia/Shanghai')

    def test_existing_localtime_is_preserved_unless_explicit(self):
        localtime = self.root / 'etc/localtime'
        localtime.write_bytes(b'existing-custom-zone')
        prepare.prepare_session_defaults(self.root)
        self.assertEqual(localtime.read_bytes(), b'existing-custom-zone')
        self.assertFalse((self.root / 'etc/timezone').exists())
        prepare.prepare_session_defaults(self.root, 'Asia/Shanghai')
        self.assertEqual(localtime.read_bytes(), b'TZif-Asia/Shanghai')

    def test_unknown_zone_and_escape_do_not_modify_configuration(self):
        for zone in ('Missing/Zone', '../../../etc/passwd'):
            with self.assertRaises(ValueError):
                prepare.prepare_session_defaults(self.root, zone)
            self.assertFalse((self.root / 'etc/localtime').exists())


if __name__ == '__main__':
    unittest.main()
