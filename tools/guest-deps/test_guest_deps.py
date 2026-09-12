"""Run with: python -m unittest discover -s tools/guest-deps -v"""
import importlib.util
import io
import json
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
import zipfile

from guest_deps import (PackageCache, ar_members, checked, elf_info,
                        linux_x86_64_member, relative_path, resolve_member,
                        sha256, tar_members)

spec = importlib.util.spec_from_file_location("prepare_root", Path(__file__).parents[1] / "prepare-root.py")
prepare_root = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare_root)


def elf(needed=(), soname="libfixture.so.1", runpath="$ORIGIN"):
    strings = bytearray(b"\0")
    tags = []
    for tag, value in [(1, value) for value in needed] + [(14, soname), (29, runpath)]:
        if value is not None:
            tags.append((tag, len(strings)))
            strings.extend(value.encode() + b"\0")
    dynamic_offset = 64 + 2 * 56
    strings_offset = dynamic_offset + (len(tags) + 3) * 16
    tags += [(5, 0x400000 + strings_offset), (10, len(strings)), (0, 0)]
    data = bytearray(strings_offset + len(strings))
    struct.pack_into("<16sHHIQQQIHHHHHH", data, 0,
                     b"\x7fELF\x02\x01\x01" + b"\0" * 9,
                     3, 62, 1, 0, 64, 0, 0, 64, 56, 2, 0, 0, 0)
    struct.pack_into("<IIQQQQQQ", data, 64, 1, 5, 0, 0x400000, 0, len(data), len(data), 4096)
    struct.pack_into("<IIQQQQQQ", data, 120, 2, 6, dynamic_offset, 0x400000 + dynamic_offset,
                     0, len(tags) * 16, len(tags) * 16, 8)
    for index, (tag, value) in enumerate(tags):
        struct.pack_into("<qQ", data, dynamic_offset + index * 16, tag, value)
    data[strings_offset:] = strings
    return bytes(data)


def tar(entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, value in entries:
            record = tarfile.TarInfo(name)
            if isinstance(value, bytes):
                record.size = len(value)
                archive.addfile(record, io.BytesIO(value))
            else:
                record.type = tarfile.SYMTYPE
                record.linkname = value
                archive.addfile(record)
    return output.getvalue()


def ar(entries):
    result = bytearray(b"!<arch>\n")
    for name, value in entries:
        result.extend(f"{name + '/':<16}{0:<12}{0:<6}{0:<6}{'100644':<8}{len(value):<10}`\n".encode())
        result.extend(value)
        if len(value) % 2:
            result.extend(b"\n")
    return bytes(result)


def package_fixture(directory, *, control_arch="amd64", source_version="1.0-1"):
    library = elf(("libc.so.6",))
    control = f"Package: libfixture1\nVersion: 1.0-1\nArchitecture: {control_arch}\nSource: fixture ({source_version})\n".encode()
    blob = ar([("debian-binary", b"2.0\n"), ("control.tar.gz", tar([("control", control)])),
               ("data.tar.gz", tar([("usr/lib/libfixture.so.1.0", library),
                                     ("usr/lib/libfixture.so.1", "libfixture.so.1.0")]))])
    record = {"package": "libfixture1", "version": "1.0-1", "architecture": "amd64",
              "source_package": "fixture", "source_version": "1.0-1",
              "url": "https://deb.debian.org/debian/pool/main/f/fixture/fixture.deb",
              "filename": "fixture.deb", "size": len(blob), "sha256": sha256(blob),
              "libraries": [{"soname": "libfixture.so.1", "member": "usr/lib/libfixture.so.1",
                             "resolved_member": "usr/lib/libfixture.so.1.0", "install_path": "usr/lib/libfixture.so.1",
                             "size": len(library), "sha256": sha256(library)}]}
    lock = directory / "lock.json"
    lock.write_text(json.dumps({"schema": 1, "guest": "linux-x86_64", "packages": [record]}), encoding="utf-8")
    cache = directory / "cache"
    cache.mkdir()
    (cache / "fixture.deb").write_bytes(blob)
    return PackageCache(lock, cache, offline=True), library


class ElfTests(unittest.TestCase):
    def test_reads_needed_soname_and_origin_without_execution(self):
        info = elf_info(elf(("libc.so.6", "libdependency.so.2")))
        self.assertEqual(info.needed, ("libc.so.6", "libdependency.so.2"))
        self.assertEqual(info.soname, "libfixture.so.1")
        self.assertEqual(info.runpaths, ("$ORIGIN",))

    def test_rejects_architecture_segment_bounds_and_bad_string_offset(self):
        original = elf(("libc.so.6",))
        for offset, fmt, value in [(18, "H", 183), (32, "Q", len(original)),
                                   (96, "Q", len(original) + 1), (184, "Q", len(original))]:
            data = bytearray(original)
            struct.pack_into("<" + fmt, data, offset, value)
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                elf_info(data)

    def test_rejects_unterminated_dynamic_and_strings(self):
        data = bytearray(elf(("libc.so.6",)))
        struct.pack_into("<q", data, 176 + 5 * 16, 1)
        with self.assertRaisesRegex(ValueError, "not terminated"):
            elf_info(data)
        data = bytearray(elf())
        data[-1] = 1
        with self.assertRaisesRegex(ValueError, "unterminated"):
            elf_info(data)

    def test_native_jar_filter_excludes_foreign_sysv_and_other_architectures(self):
        native = elf()
        self.assertTrue(linux_x86_64_member("linux-x86-64/libfixture.so", native))
        for name in ("dragonflybsd-x86-64/libfixture.so", "freebsd-x86-64/libfixture.so",
                     "win32-x86-64/windows/libfixture.so", "android-x86-64/libfixture.so"):
            self.assertFalse(linux_x86_64_member(name, native))
        arm = bytearray(native)
        struct.pack_into("<H", arm, 18, 183)
        self.assertFalse(linux_x86_64_member("linux-aarch64/libfixture.so", arm))


class ArchiveTests(unittest.TestCase):
    def test_posix_only_unselected_names_do_not_block_verified_payloads(self):
        with tempfile.TemporaryDirectory() as temporary:
            cache, library = package_fixture(Path(temporary))
            package = cache.packages['libfixture1']
            path = cache.directory / package['filename']
            entries = ar_members(path.read_bytes())
            manual = 'usr/share/man/File::Find.3pm.gz'
            unit = r'lib/systemd/system/system-systemd\x2dcryptsetup.slice'
            entries['data.tar.gz'] = tar([
                ('usr/lib/libfixture.so.1.0', library),
                ('usr/lib/libfixture.so.1', 'libfixture.so.1.0'),
                (manual, b'manual'), (unit, b'unit'),
            ])
            blob = ar(entries.items())
            path.write_bytes(blob)
            package.update(size=len(blob), sha256=sha256(blob))
            files, _ = cache.materialize('libfixture1')
            self.assertEqual(set(files), {'usr/lib/libfixture.so.1'})
            for member, content in ((manual, b'manual'), (unit, b'unit')):
                cache.loaded.clear()
                package['files'] = [{'member': member, 'install_path': member,
                                     'size': len(content), 'sha256': sha256(content)}]
                with self.subTest(member=member), self.assertRaises(ValueError):
                    cache.materialize('libfixture1')

    def test_lookup_alias_preserves_authenticated_elf_soname(self):
        with tempfile.TemporaryDirectory() as temporary:
            cache, library = package_fixture(Path(temporary))
            record = cache.packages['libfixture1']['libraries'][0]
            record['name'] = 'libfixture.so.0'
            record['install_path'] = 'usr/lib/libfixture.so.0'
            cache.lock_path.write_text(json.dumps(cache.lock), encoding='utf-8')
            loaded = PackageCache(cache.lock_path, cache.directory, offline=True)
            path, install, _, _ = loaded.resolve('libfixture.so.0')
            self.assertEqual(path.read_bytes(), library)
            self.assertEqual(install, 'usr/lib/libfixture.so.0')
            self.assertIsNone(loaded.resolve('libfixture.so.1'))
            record['soname'] = 'libfixture.so.0'
            cache.lock_path.write_text(json.dumps(cache.lock), encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'ELF SONAME disagrees'):
                PackageCache(cache.lock_path, cache.directory, offline=True).resolve('libfixture.so.0')
            record['name'] = '../libfixture.so.0'
            cache.lock_path.write_text(json.dumps(cache.lock), encoding='utf-8')
            with self.assertRaises(ValueError):
                PackageCache(cache.lock_path, cache.directory, offline=True)

    def test_architecture_independent_data_package_still_checks_control_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            cache, _ = package_fixture(Path(temporary), control_arch="all")
            package = cache.packages['libfixture1']
            package['architecture'] = 'all'
            archive = ar_members((cache.directory / package['filename']).read_bytes())
            content = b'tcp 6 TCP\n'
            archive['data.tar.gz'] = tar([('etc/protocols', content)])
            blob = ar(list(archive.items()))
            package.update(size=len(blob), sha256=sha256(blob), libraries=[], files=[{
                'member': 'etc/protocols', 'install_path': 'etc/protocols',
                'size': len(content), 'sha256': sha256(content)}])
            (cache.directory / package['filename']).write_bytes(blob)
            cache.lock_path.write_text(json.dumps(cache.lock), encoding='utf-8')
            loaded = PackageCache(cache.lock_path, cache.directory, offline=True)
            files, provenance = loaded.materialize('libfixture1')
            self.assertEqual(files['etc/protocols'].read_bytes(), content)
            self.assertEqual(provenance['architecture'], 'all')
            package['architecture'] = 'amd64'
            cache.lock_path.write_text(json.dumps(cache.lock), encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'Architecture disagrees'):
                PackageCache(cache.lock_path, cache.directory, offline=True).materialize('libfixture1')
            package['architecture'] = 'arm64'
            cache.lock_path.write_text(json.dumps(cache.lock), encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'not amd64 or all'):
                PackageCache(cache.lock_path, cache.directory, offline=True)

    def test_absolute_package_links_stay_in_guest_member_map(self):
        members = tar_members(tar([('etc/settings', b'guest config'),
                                   ('usr/lib/settings', '/etc/settings')]))
        self.assertEqual(resolve_member(members, 'usr/lib/settings'), ('etc/settings', b'guest config'))
        with self.assertRaises(KeyError):
            resolve_member(tar_members(tar([('usr/lib/settings', '/etc/absent')])), 'usr/lib/settings')
        with self.assertRaises(ValueError):
            tar_members(tar([('usr/lib/settings', '/../../host')]))

    def test_package_trees_are_selected_verified_and_repair_cached_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            cache, library = package_fixture(Path(temporary))
            package = cache.packages['libfixture1']
            path = cache.directory / package['filename']
            entries = ar_members(path.read_bytes())
            entries['data.tar.gz'] = tar([
                ('usr/lib/libfixture.so.1.0', library),
                ('usr/lib/libfixture.so.1', 'libfixture.so.1.0'),
                ('usr/lib/python3.11/module.py', b'answer = 42\n'),
                ('usr/lib/python3.11/config.py', '/etc/config.py'),
                ('usr/lib/python3.11/cross-package.py', '/missing.py'),
                ('etc/config.py', b'configured = True\n'),
                ('usr/lib/python3.110/outside.py', b'not selected'),
            ])
            blob = ar(entries.items())
            path.write_bytes(blob)
            package.update(size=len(blob), sha256=sha256(blob), trees=[{
                'member': 'usr/lib/python3.11', 'install_path': 'usr/lib/python3.11',
                'exclude': ['usr/lib/python3.11/cross-package.py'],
            }])
            cache.lock_path.write_text(json.dumps(cache.lock), encoding='utf-8')
            files, provenance = cache.materialize('libfixture1')
            self.assertEqual(set(files), {'usr/lib/libfixture.so.1',
                                         'usr/lib/python3.11/module.py', 'usr/lib/python3.11/config.py'})
            self.assertEqual(files['usr/lib/python3.11/config.py'].read_bytes(), b'configured = True\n')
            self.assertEqual(provenance['sha256'], sha256(blob))
            files['usr/lib/python3.11/module.py'].write_bytes(b'tampered')
            fresh = PackageCache(cache.lock_path, cache.directory, offline=True)
            restored, _ = fresh.materialize('libfixture1')
            self.assertEqual(restored['usr/lib/python3.11/module.py'].read_bytes(), b'answer = 42\n')
            with self.assertRaisesRegex(ValueError, 'absent'):
                cache.materialize('unreviewed')
            for tree in [{'member': '../escape', 'install_path': 'usr/lib'},
                         {'member': 'absent', 'install_path': 'usr/lib'},
                         {'member': 'usr/lib/python3.11', 'install_path': 'usr/lib',
                          'exclude': ['etc/config.py']}]:
                fresh = PackageCache(cache.lock_path, cache.directory, offline=True)
                fresh.packages['libfixture1']['trees'] = [tree]
                with self.subTest(tree=tree), self.assertRaises(ValueError):
                    fresh.materialize('libfixture1')

    def test_rejects_path_traversal_and_link_cycles(self):
        for value in ("../outside", "/absolute", "C:/outside", "a\\b", "a\0b", ""):
            with self.subTest(path=value), self.assertRaises(ValueError):
                relative_path(value)
        with self.assertRaises(ValueError):
            tar_members(tar([("usr/lib/link", "../../../outside")]))
        with self.assertRaises(ValueError):
            tar_members(tar([("../outside", b"data")]))
        members = tar_members(tar([("one", "two"), ("two", "one")]))
        with self.assertRaisesRegex(ValueError, "cycle"):
            resolve_member(members, "one")

    def test_rejects_truncated_or_duplicate_archives_and_wrong_hashes(self):
        valid = ar([("debian-binary", b"2.0\n")])
        with self.assertRaises(ValueError):
            ar_members(valid[:-1])
        with self.assertRaises(ValueError):
            ar_members(ar([("debian-binary", b"2.0\n"), ("debian-binary", b"2.0\n")]))
        with self.assertRaises(ValueError):
            tar_members(tar([("same", b"one"), ("same", b"two")]))
        with self.assertRaisesRegex(ValueError, "SHA-256"):
            checked(b"same size", {"size": 9, "sha256": sha256(b"different")}, "tampered")

    def test_offline_package_checks_control_and_materializes_real_elf(self):
        with tempfile.TemporaryDirectory() as temporary:
            cache, library = package_fixture(Path(temporary))
            path, target, provenance, documents = cache.resolve("libfixture.so.1")
            self.assertEqual(path.read_bytes(), library)
            self.assertFalse(path.is_symlink())
            self.assertEqual(target, "usr/lib/libfixture.so.1")
            self.assertEqual(provenance["source_package"], "fixture")
            self.assertEqual(documents, [])
            self.assertIsNone(cache.resolve("libunreviewed.so"))
            path.write_bytes(b"tampered materialization")
            fresh = PackageCache(cache.lock_path, cache.directory, offline=True)
            self.assertEqual(fresh.resolve("libfixture.so.1")[0].read_bytes(), library)

    def test_control_architecture_source_version_and_archive_corruption_fail(self):
        for values in ({"control_arch": "arm64"}, {"source_version": "2.0-1"}, {}):
            with tempfile.TemporaryDirectory() as temporary:
                cache, _ = package_fixture(Path(temporary), **values)
                if not values:
                    (cache.directory / "fixture.deb").write_bytes(b"damaged")
                with self.subTest(values=values), self.assertRaises(ValueError):
                    cache.resolve("libfixture.so.1")


class RootPlanTests(unittest.TestCase):
    def test_reinstall_preserves_identical_mapped_payloads(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary).resolve()
            cache, _ = package_fixture(base)
            source, root = base / 'source', base / 'root'
            source.mkdir()
            payload = source / 'payload'
            payload.write_bytes(b'reviewed resource')
            plan = prepare_root.RootPlan(source, root, cache, set(), 'jre')
            plan.add(payload, 'usr/share/payload')
            plan.install(['resource'])
            from unittest.mock import patch
            with patch.object(prepare_root.shutil, 'copyfile', side_effect=AssertionError('unnecessary overwrite')):
                plan.install(['resource'])
            payload.write_bytes(b'updated resource')
            plan.install(['resource'])
            self.assertEqual((root / 'usr/share/payload').read_bytes(), b'updated resource')

    def test_implicit_library_dependency_keeps_its_runtime_resources(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary).resolve()
            cache, library = package_fixture(base)
            package = cache.packages['libfixture1']
            archive = cache.directory / package['filename']
            entries = ar_members(archive.read_bytes())
            entries['data.tar.gz'] = tar([
                ('usr/lib/libfixture.so.1.0', library),
                ('usr/lib/libfixture.so.1', 'libfixture.so.1.0'),
                ('usr/share/fixture/schema.xml', b'<schema id="fixture"/>'),
            ])
            blob = ar(entries.items())
            archive.write_bytes(blob)
            package.update(size=len(blob), sha256=sha256(blob), trees=[{
                'member': 'usr/share/fixture', 'install_path': 'usr/share/fixture',
            }])
            cache.lock_path.write_bytes(json.dumps(cache.lock).encode('utf-8'))
            source, root = base / 'source', base / 'root'
            source.mkdir()
            program = source / 'app'
            program.write_bytes(elf(('libfixture.so.1',), soname=None))
            plan = prepare_root.RootPlan(source, root, cache, {'libc.so.6'}, 'jre')
            plan.add(program, 'usr/bin/app')
            plan.close_dependencies()
            plan.install(['app'])
            self.assertEqual((root / 'usr/share/fixture/schema.xml').read_bytes(),
                             b'<schema id="fixture"/>')
            self.assertEqual((root / 'usr/lib/libfixture.so.1').read_bytes(), library)

    def test_package_cycle_stops_before_root_install(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary).resolve()
            cache, _ = package_fixture(base)
            cache.packages['libfixture1']['requires'] = ['libfixture1']
            source, root = base / 'source', base / 'root'
            source.mkdir()
            plan = prepare_root.RootPlan(source, root, cache, {'libc.so.6'}, 'jre')
            with self.assertRaisesRegex(ValueError, 'cycle'):
                plan.copy_package('libfixture1')
            self.assertFalse(root.exists())

    def test_explicit_package_owns_dependency_resolution(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary).resolve()
            cache, library = package_fixture(base)
            source, root = base / 'source', base / 'root'
            (source / 'usr/lib').mkdir(parents=True)
            (source / 'usr/lib/libfixture.so.1').write_bytes(b'old invalid source')
            app = source / 'app'
            app.write_bytes(elf(('libfixture.so.1',), soname=None))
            plan = prepare_root.RootPlan(source, root, cache, {'libc.so.6'}, 'jre')
            plan.add(app, 'usr/bin/app')
            plan.copy_package('libfixture1')
            plan.close_dependencies()
            plan.install(['package:libfixture1'])
            self.assertEqual((root / 'usr/lib/libfixture.so.1').read_bytes(), library)
            edge = next(edge for edge in plan.edges if edge['needed'] == 'libfixture.so.1')
            self.assertEqual(edge['package'], 'libfixture1')

    def test_missing_dependency_preflight_leaves_root_untouched(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary).resolve()
            cache, _ = package_fixture(base)
            source, root = base / "source", base / "root"
            source.mkdir()
            app = source / "app"
            app.write_bytes(elf(("libmissing.so",)))
            plan = prepare_root.RootPlan(source, root, cache, {"libc.so.6"}, "jre")
            plan.add(app, "usr/bin/app")
            with self.assertRaisesRegex(FileNotFoundError, "libmissing.so"):
                plan.close_dependencies()
            self.assertFalse(root.exists())

    def test_compiler_relocatable_objects_are_payloads_without_runtime_dependencies(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary).resolve()
            cache, _ = package_fixture(base)
            source, root = base / 'source', base / 'root'
            source.mkdir()
            data = bytearray(64)
            data[:7] = b'\x7fELF\x02\x01\x01'
            struct.pack_into('<HH', data, 16, 1, 62)
            obj = source / 'crtbegin.o'
            obj.write_bytes(data)
            plan = prepare_root.RootPlan(source, root, cache, set(), 'jre')
            plan.add(obj, 'usr/lib/crtbegin.o')
            plan.close_dependencies()
            self.assertEqual(len(plan.edges), 0)
            plan.install(['compiler'])
            self.assertEqual((root / 'usr/lib/crtbegin.o').read_bytes(), data)

    def test_closes_jars_and_package_dependencies_and_records_hashes(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary).resolve()
            cache, library = package_fixture(base)
            source, root = base / "source", base / "root"
            source.mkdir()
            archive = source / "natives.jar"
            with zipfile.ZipFile(archive, "w") as jar:
                jar.writestr("linux-x86-64/libnative.so", elf(("libfixture.so.1",)))
                jar.writestr("dragonflybsd-x86-64/libnative.so", elf(("libc.so.8",)))
            plan = prepare_root.RootPlan(source, root, cache, {"libc.so.6"}, "jre")
            plan.add(archive, "minecraft/natives.jar", inspect_jars=True)
            plan.close_dependencies()
            self.assertEqual(len(plan.native_members), 1)
            self.assertEqual(len(plan.edges), 2)
            report_path = base / 'root-provenance.json'
            self.assertEqual(plan.install(["minecraft"], report_path), 2)
            self.assertEqual((root / "usr/lib/libfixture.so.1").read_bytes(), library)
            self.assertFalse((root / "kinakaze-artifacts.json").exists())
            report = json.loads(report_path.read_text(encoding="utf-8"))
            records = {record["path"]: record for record in report["files"]}
            self.assertEqual(records["usr/lib/libfixture.so.1"]["sha256"], sha256(library))
            self.assertEqual(report["dependency_lock_sha256"], sha256(cache.lock_path.read_bytes()))


if __name__ == "__main__":
    unittest.main()
