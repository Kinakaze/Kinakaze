"""Guest linkers require ELF interfaces in both online and offline manifests."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

TOOLS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(TOOLS))
spec = importlib.util.spec_from_file_location('release_rootfs', TOOLS / 'prepare-release-rootfs.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class InstallerBoundary(Exception):
    pass


class LinkerInterfaceTests(unittest.TestCase):
    def test_linker_inputs_and_executable_loader_stay_distinct_in_both_manifests(self):
        for online in (False, True):
            with self.subTest(online=online), tempfile.TemporaryDirectory() as temporary:
                base = Path(temporary)
                dist, interfaces = base / 'dist', base / 'interfaces'
                (dist / 'rootfs/lib').mkdir(parents=True)
                copyright = dist / 'rootfs/usr/share/doc/kinakaze-libc/copyright'
                copyright.parent.mkdir(parents=True)
                copyright.write_bytes(b'fixture copyright')
                interfaces.mkdir()
                names = ('libc.so.6', 'ld-linux-x86-64.so.2')
                for name in names:
                    (dist / 'rootfs/lib' / name).write_bytes(b'MZ-native-' + name.encode())
                    (interfaces / name).write_bytes(b'\x7fELF-linker-' + name.encode())
                preset = base / 'preset.json'
                preset.write_text(json.dumps(dict(schema=1, debian_standard_lock='lock.json',
                                                  busybox_applets=[], native_packages=[])))
                (base / 'lock.json').write_text(json.dumps(dict(selection={})))
                adaptations = [dict(path=path, reason='native ABI provider') for path in
                               ('lib/x86_64-linux-gnu/libc.so.6', 'lib64/ld-linux-x86-64.so.2')]
                scripts = {'usr/lib/x86_64-linux-gnu/libc.so':
                           b'GROUP ( /lib/x86_64-linux-gnu/libc.so.6 AS_NEEDED ( /lib64/ld-linux-x86-64.so.2 ) )'}
                cache = SimpleNamespace(packages={}, directory=base / 'cache', offline=True)
                with (patch.object(release, 'modules', return_value=names),
                      patch.object(release, 'load_standard', return_value=(scripts, {}, {}, set(), [], adaptations)),
                      patch.object(release, 'configure_standard'),
                      patch.object(release, 'configure_packages'),
                      patch.object(release.subprocess, 'run', side_effect=InstallerBoundary)):
                    if online:
                        release.prepare(dist, cache, preset, online=True, elf_imports=interfaces)
                    else:
                        # Stop at the native installer boundary after manifest creation.
                        with self.assertRaises(InstallerBoundary):
                            release.prepare(dist, cache, preset, elf_imports=interfaces)
                manifest = json.loads((dist / 'rootfs.manifest.json').read_text())
                files = {entry['path']: entry for entry in manifest['files']}
                for name in names:
                    prefix = 'usr/lib' if name.startswith('ld-') else 'lib'
                    target = f'{prefix}/x86_64-linux-gnu/{name}'
                    self.assertNotIn(target, manifest['links'])
                    self.assertEqual((dist / files[target]['source']).read_bytes(),
                                     (interfaces / name).read_bytes())
                    self.assertEqual((dist / files['lib/' + name]['source']).read_bytes(),
                                     b'MZ-native-' + name.encode())
                self.assertEqual(manifest['links']['lib64/ld-linux-x86-64.so.2'],
                                 '/lib/ld-linux-x86-64.so.2')
                entry = files['usr/lib/x86_64-linux-gnu/libc.so']
                script = (dist / entry['source']).read_bytes() if 'source' in entry else entry['content'].encode()
                self.assertIn(b'/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2', script)
                self.assertNotIn(b'/lib64/ld-linux-x86-64.so.2', script)


if __name__ == '__main__':
    unittest.main()
