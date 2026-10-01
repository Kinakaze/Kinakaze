"""APT's component version view must not invent a persistent Debian install."""
from pathlib import Path
import json
import sys
import unittest

WORKSPACE = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(WORKSPACE / 'tools'))
from rootfs_bootstrap import preserve_debconf_extractor

manifest = json.loads((WORKSPACE / 'config/rootfs.manifest.json').read_text(encoding='utf-8'))
adapter_source = next(entry['content'] for entry in manifest['files']
                      if entry['path'] == 'usr/bin/apt-extracttemplates')
adapter = {'__name__': 'apt_extracttemplates'}
exec(compile(adapter_source, 'rootfs.manifest.json:usr/bin/apt-extracttemplates', 'exec'), adapter)
status_view = adapter['status_view']
configuration_options = adapter['configuration_options']
BASE = (b'Package: kinakaze-base\nStatus: install ok installed\nVersion: 1.0.0\n'
        b'Provides: apt (= 2.6.1),\n debconf (= 1.5.82), perl-base (= 5.36.0)\n\n')


class DebconfBootstrapTests(unittest.TestCase):
    def test_view_preserves_real_packages_and_uses_the_component_version(self):
        status = BASE + b'Package: example\nStatus: install ok installed\nVersion: 2\n\n'
        view = status_view(status)
        self.assertTrue(view.startswith(status))
        self.assertIn(b'Package: debconf\nStatus: install ok installed\nArchitecture: all\nVersion: 1.5.82\n', view)
        self.assertNotIn(b'Package: debconf\n', status)
        self.assertEqual(status_view(status.replace(b'\n', b'\r\n')).count(b'Package: debconf\n'), 1)

    def test_real_debconf_records_take_precedence_in_every_installation_state(self):
        for state in ('installed', 'unpacked', 'half-configured', 'config-files'):
            debconf = f'Package: debconf\nStatus: install ok {state}\nVersion: 1.5.99\n\n'.encode()
            self.assertIsNone(status_view(BASE + debconf))
            self.assertIsNone(status_view(debconf + BASE))

    def test_absent_unversioned_or_uninstalled_provider_is_not_fabricated(self):
        for data in (b'', BASE.replace(b'debconf (= 1.5.82)', b'debconf'),
                     BASE.replace(b'install ok installed', b'deinstall ok config-files'),
                     BASE.replace(b'debconf (= 1.5.82)', b'debconf-data (= 1.5.82)')):
            self.assertIsNone(status_view(data))

    def test_configuration_flags_and_separator_are_preserved(self):
        args = ['-t', '/tmp', '-o', 'Dir::State::status=/a path/status', '-c/custom',
                '--option=A=B', '--config-file', '/custom file', '--', '-oarchive.deb']
        self.assertEqual(configuration_options(args),
                         ['-o', 'Dir::State::status=/a path/status', '-c/custom',
                          '--option=A=B', '--config-file', '/custom file'])
        self.assertIsNone(configuration_options(['--option']))

    def test_packaging_retains_original_extractor_before_manifest_overlay(self):
        overlays = {'usr/bin/apt-extracttemplates': adapter_source.encode()}
        files = {'usr/bin/apt-extracttemplates': b'\x7fELF-original'}
        preserve_debconf_extractor(files, overlays)
        files.update(overlays)
        self.assertEqual(files['usr/lib/kinakaze/apt-extracttemplates'], b'\x7fELF-original')
        self.assertTrue(files['usr/bin/apt-extracttemplates'].startswith(b'#!/usr/bin/python3\n'))
        before = dict(files)
        preserve_debconf_extractor(files, {})
        self.assertEqual(files, before)
        with self.assertRaises(ValueError):
            preserve_debconf_extractor({'usr/bin/apt-extracttemplates': b'unknown executable'}, overlays)
        with self.assertRaises(ValueError):
            preserve_debconf_extractor(files, overlays)


if __name__ == '__main__':
    unittest.main()
