"""CLI version discovery must reject ambiguity rather than pick an old build."""
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from guest_installation import java_home, minecraft_version


class InstallationTests(unittest.TestCase):
    def test_java_discovery_uses_installed_directory_and_rejects_multiple_versions(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaises(ValueError):
                java_home(root)
            java = root / 'usr/lib/jvm/custom-jre/bin/java'
            java.parent.mkdir(parents=True)
            java.write_bytes(b'fixture')
            self.assertEqual(java_home(root), 'usr/lib/jvm/custom-jre')
            other = root / 'usr/lib/jvm/other/bin/java'
            other.parent.mkdir(parents=True)
            other.write_bytes(b'fixture')
            with self.assertRaises(ValueError):
                java_home(root)

    def test_minecraft_discovery_requires_version_metadata(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            directory = root / 'minecraft/versions/custom'
            directory.mkdir(parents=True)
            with self.assertRaises(ValueError):
                minecraft_version(root)
            (directory / 'custom.json').write_text('{}', encoding='utf-8')
            self.assertEqual(minecraft_version(root), 'custom')
            other = directory.parent / 'other'
            other.mkdir()
            (other / 'other.json').write_text('{}', encoding='utf-8')
            with self.assertRaises(ValueError):
                minecraft_version(root)
