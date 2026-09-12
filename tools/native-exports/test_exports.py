"""Native export tests, independent of a Windows loader or a Cargo build."""
import json
from pathlib import Path
import struct
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

import generate


def write_evidence(root, checked):
    inventory = {(row["soname"], row["name"]): set(row["versions"])
                 for row in checked["observed_versions"]}
    for path, text in generate.abi_evidence.outputs(root, inventory, checked["elf_import_evidence"]).items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


class ExportTests(unittest.TestCase):
    def test_legacy_input_device_functions_belong_to_libxi(self):
        for name in ("XOpenDevice", "XCloseDevice", "XSetDeviceMode", "XSetDeviceButtonMapping",
                     "XListInputDevices", "XFreeDeviceList", "XGrabDevice", "XUngrabDevice",
                     "XSelectExtensionEvent", "XGetDeviceMotionEvents", "XFreeDeviceMotionEvents",
                     "XQueryDeviceState", "XFreeDeviceState", "XGetDeviceProperty"):
            self.assertTrue(generate.belongs("libXi", name), name)
            self.assertFalse(generate.belongs("libX11", name), name)

    def test_missing_assembly_export_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'src').mkdir()
            (root / 'src/variadic.rs').write_text('.globl kinakaze_abi_error_at_line\n', encoding='utf-8')
            with patch.object(generate, 'source_directory', return_value=root):
                with self.assertRaisesRegex(ValueError, 'kinakaze_abi_error_at_line is missing'):
                    generate.validate_literal_exports('libc', {})
                generate.validate_literal_exports('libc', {'kinakaze_abi_error_at_line': 'T'})

    def test_compatibility_exports_share_existing_implementation_targets(self):
        exports = {"libc": {name: {"runtime_export": "libc_" + name} for name in generate.PTHREAD_LIBC_FORWARDERS | {"__tls_get_addr", "__libc_stack_end", "_exit", "vfscanf", "vsscanf", "scanf", "vscanf", "clearerr", "__pread_chk", "stpcpy", "mempcpy", "dcgettext", "__strtod_l", "__strtof_l", "ftw", "nftw", "versionsort", "fgetpos", "fsetpos", "__fread_chk", "strndup", "__strftime_l"}},
                   "libpthread": {name: {"runtime_export": "pthread_" + name} for name in generate.LIBC_PTHREAD_FORWARDERS},
                   "libm": {name: {"runtime_export": "math_" + name} for name in generate.LIBC_LIBM_FORWARDERS | {'lrint', 'lrintf', 'remainder', 'lgamma', 'atan2', 'exp', 'fpclassifyf', 'fpclassify'}},
                   "librt": {name: {"runtime_export": "rt_" + name} for name in generate.LIBC_RT_FORWARDERS},
                   "ld-linux-x86-64": {}}
        generate.add_compatibility_exports(exports)
        for name in generate.PTHREAD_LIBC_FORWARDERS | generate.LIBC_PTHREAD_FORWARDERS:
            self.assertEqual(exports['libc'][name], exports['libpthread'][name])
        for name in generate.LIBC_RT_FORWARDERS:
            self.assertEqual(exports['libc'][name], exports['librt'][name])
        for name in generate.LIBC_LIBM_FORWARDERS:
            self.assertEqual(exports['libc'][name], exports['libm'][name])
        for alias, original in {'llrint': 'lrint', 'llrintf': 'lrintf', 'drem': 'remainder', 'gamma': 'lgamma', '__atan2_finite': 'atan2', '__exp_finite': 'exp', '__fpclassifyf': 'fpclassifyf'}.items():
            self.assertEqual(exports['libm'][alias]['runtime_export'], exports['libm'][original]['runtime_export'])
            self.assertEqual(exports['libm'][alias]['name'], alias)
        for alias, original in {'_Exit': '_exit', '__isoc99_vfscanf': 'vfscanf',
                                '__isoc99_vsscanf': 'vsscanf', '__isoc99_scanf': 'scanf', '__isoc99_vscanf': 'vscanf', 'clearerr_unlocked': 'clearerr', '__pread64_chk': '__pread_chk', '__stpcpy': 'stpcpy', '__mempcpy': 'mempcpy', '__dcgettext': 'dcgettext', 'strtod_l': '__strtod_l', 'strtof_l': '__strtof_l', 'ftw64': 'ftw', 'nftw64': 'nftw', 'versionsort64': 'versionsort', 'fgetpos64': 'fgetpos', 'fsetpos64': 'fsetpos', '__fread_unlocked_chk': '__fread_chk'}.items():
            self.assertEqual(exports['libc'][alias]['runtime_export'], exports['libc'][original]['runtime_export'])
            self.assertEqual(exports['libc'][alias]['name'], alias)
        with self.assertRaisesRegex(ValueError, 'no libc implementation'):
            generate.add_compatibility_exports({'libc': {}, 'libpthread': {}})

    def test_extension_preserves_prior_versions_and_deduplicates_evidence(self):
        old = {"path": "old.so", "sha256": "old-hash"}
        new = {"path": "new.so", "sha256": "new-hash"}
        checked = {"observed_versions": [{"soname": "libc.so.6", "name": "pthread_sigmask", "versions": ["GLIBC_2.2.5"]}],
                   "elf_import_evidence": [old]}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_evidence(root, checked)
            with patch.object(generate, "ROOT", root), patch.object(generate, "import_inventory", return_value=(
                    {("libc.so.6", "pthread_sigmask"): {"GLIBC_2.32"}}, [old, new, new])):
                inventory, evidence = generate.version_evidence(SimpleNamespace(observe=None, refresh_evidence=False, extend_evidence=[root]))
        self.assertEqual(inventory[("libc.so.6", "pthread_sigmask")], {"GLIBC_2.2.5", "GLIBC_2.32"})
        self.assertEqual(evidence, [old, new])

    def test_empty_refresh_preserves_checked_version_evidence(self):
        before = (generate.ROOT / "tools/abi/versions.tsv").read_bytes()
        with patch.object(generate, "default_observation_paths", return_value=[]):
            with self.assertRaisesRegex(ValueError, "No versioned ELF evidence"):
                generate.version_evidence(SimpleNamespace(observe=None, refresh_evidence=True))
        self.assertEqual((generate.ROOT / "tools/abi/versions.tsv").read_bytes(), before)

    def test_directory_and_jar_observations_filter_architecture_without_extracting(self):
        x64 = bytearray(64)
        x64[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", x64, 18, 62)
        arm64 = bytearray(x64)
        struct.pack_into("<H", arm64, 18, 183)
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            archive = directory / "natives-linux.jar"
            with zipfile.ZipFile(archive, "w") as output:
                output.writestr("linux/x64/library.so", x64)
                output.writestr("linux/arm64/library.so", arm64)
                output.writestr("openbsd-x86-64/library.so", x64)
                output.writestr("windows/library.dll", b"MZ")
                output.writestr("example/Example.class", b"\xca\xfe\xba\xbe")
            (directory / "standalone.elf").write_bytes(x64)
            observed = list(generate.observed_elfs([directory]))
            self.assertEqual(len(observed), 2)
            members = [entry for _, entry in observed if "member" in entry]
            self.assertEqual(members[0]["member"], "linux/x64/library.so")
            self.assertEqual(len(members[0]["archive_sha256"]), 64)
            self.assertFalse((directory / "linux").exists())

    def test_normal_generation_uses_checked_versions_without_guest_files(self):
        checked = {
            "observed_versions": [{"soname": "libc.so.6", "name": "jrand48", "versions": ["GLIBC_2.2.5"]}],
            "elf_import_evidence": [{"path": "unavailable/libsasl2.so.2", "sha256": "checked-evidence"}],
        }
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_evidence(root, checked)
            with patch.object(generate, "ROOT", root), patch.object(
                    generate, "import_inventory", side_effect=AssertionError("must not read guest files")):
                observed, evidence = generate.version_evidence(SimpleNamespace(observe=None, refresh_evidence=False))
        self.assertEqual(observed[("libc.so.6", "jrand48")], {"GLIBC_2.2.5"})
        self.assertEqual(evidence, checked["elf_import_evidence"])

    def test_aliases_share_one_data_target_and_payload(self):
        result = generate.guest_exports("libc", {"kinakaze_abi_environ": "D"},
                                        [("__environ", "kinakaze_abi_environ")])
        self.assertEqual(result["environ"]["runtime_export"], result["__environ"]["runtime_export"])
        self.assertEqual((result["__environ"]["kind"], result["__environ"]["size"]), ("object", 8))

    def test_source_alias_must_have_actual_compiled_definition(self):
        with self.assertRaisesRegex(ValueError, "uncompiled symbol"):
            generate.guest_exports("libc", {}, [("fake", "kinakaze_abi_missing")])

    def test_loader_aliases_require_compiled_entries_and_one_owner(self):
        definitions = {f"kinakaze_process_{name}": "T" for name in generate.LOADER_EXPORTS}
        exports = {"ld-linux-x86-64": generate.guest_exports("ld-linux-x86-64", definitions), "libc": {}}
        generate.add_loader_exports(exports)
        self.assertEqual(exports["libc"], exports["ld-linux-x86-64"])
        with self.assertRaisesRegex(ValueError, "Duplicate libc implementation"):
            generate.add_loader_exports(exports)
        del definitions["kinakaze_process_dlsym"]
        with self.assertRaisesRegex(ValueError, "uncompiled symbol"):
            generate.guest_exports("ld-linux-x86-64", definitions)

    def test_unknown_data_cannot_turn_into_a_function(self):
        with self.assertRaisesRegex(ValueError, "explicit guest ABI layout"):
            generate.guest_exports("libc", {"kinakaze_abi_unknown_global": "D"})

    def test_private_redirect_bookkeeping_is_not_exported(self):
        result = generate.guest_exports("libc", {"kinakaze_abi_optind": "D"})
        self.assertEqual((result["optind"]["size"], result["optind"]["alignment"]), (4, 4))

    def test_internal_helpers_are_not_guest_functions(self):
        result = generate.guest_exports("libc", {
            "kinakaze_abi_malloc_usable_size": "T", "malloc_usable_size": "T",
            "kinakaze_thread_fs_adopt": "T", "_ZN4core_internal": "T",
        })
        self.assertEqual(list(result), ["malloc_usable_size"])
        self.assertEqual(result["malloc_usable_size"]["runtime_export"], "kinakaze_abi_malloc_usable_size")

    def test_xsync_core_symbol_is_in_x11(self):
        self.assertTrue(generate.belongs("libX11", "XSync"))
        self.assertTrue(generate.belongs("libX11", "XSynchronize"))
        self.assertFalse(generate.belongs("libXext", "XSync"))
        self.assertFalse(generate.belongs("libXext", "XSynchronize"))
        self.assertFalse(generate.belongs("libX11", "XSyncCreateCounter"))
        self.assertTrue(generate.belongs("libXext", "XSyncCreateCounter"))

    def test_xinput_prefix_does_not_capture_core_xlib_functions(self):
        for name in ["XInitThreads", "XInternAtom", "XInternAtoms", "XInitExtension",
                     "XIfEvent", "XIconifyWindow", "XIntersectRegion", "XIMOfIC"]:
            self.assertTrue(generate.belongs("libX11", name), name)
            self.assertFalse(generate.belongs("libXi", name), name)
        for name in ["XIQueryVersion", "XISelectEvents", "XIQueryDevice"]:
            self.assertTrue(generate.belongs("libXi", name), name)
            self.assertFalse(generate.belongs("libX11", name), name)

    def test_x11_extension_families_have_exactly_one_owner(self):
        # Extension ownership survives moving its code into its own native SO.
        families = [name for name in generate.INPUTS if name.startswith('libX')]
        for name, expected in [
            ("XInitThreads", "libX11"), ("XSynchronize", "libX11"),
            ("XRRGetScreenResources", "libXrandr"), ("XRenderCreatePicture", "libXrender"),
            ("XcursorImageCreate", "libXcursor"), ("XineramaQueryScreens", "libXinerama"),
            ("XIQueryVersion", "libXi"), ("XF86VidModeQueryVersion", "libXxf86vm"),
            ("XShapeQueryExtension", "libXext"), ("XGetXCBConnection", "libX11-xcb"),
        ]:
            self.assertEqual([family for family in families if generate.belongs(family, name)], [expected], name)

    def test_input_and_randr_are_direct_native_owners(self):
        for name in ('libXi', 'libXrandr'):
            self.assertTrue(generate.INPUTS[name]['native'])
            self.assertEqual(generate.INPUTS[name]['implementation'], name)

    def test_glx_and_egl_have_separate_frontends(self):
        self.assertTrue(generate.belongs("libGLX", "glXCreateContext"))
        self.assertFalse(generate.belongs("libGLX", "glClear"))
        self.assertTrue(generate.belongs("libEGL", "eglInitialize"))
        self.assertFalse(generate.belongs("libEGL", "glXCreateContext"))

    def test_loader_frontend_has_a_distinct_audited_surface(self):
        self.assertTrue(generate.belongs("ld-linux-x86-64", "__tls_get_addr"))
        self.assertFalse(generate.belongs("ld-linux-x86-64", "malloc"))
        self.assertFalse(generate.belongs("ld-linux-x86-64", "_rtld_global"))

    def test_native_linker_names_are_unique(self):
        modules = list(generate.INPUTS.values())
        self.assertEqual(len(modules), len({module['library'].lower() for module in modules}))
        self.assertEqual(len(modules), len({module['soname'] for module in modules}))

    def test_observed_copy_objects_keep_version_requirement(self):
        # Minimal ELF with .dynstr, .dynsym, .gnu.version and .gnu.version_r.
        # A COPY object has a defined section index, unlike undefined functions.
        strings = b"\0libc.so.6\0GLIBC_2.2.5\0optind\0"
        symbols = bytes(24) + struct.pack("<IBBHQQ", 23, 0x11, 0, 7, 0x404000, 4)
        versym = struct.pack("<HH", 0, 2)
        need = struct.pack("<HHIII", 1, 1, 1, 16, 0)
        need += struct.pack("<IHHII", 0, 0, 2, 11, 0)
        data = bytearray(64 + 5 * 64)
        data[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", data, 18, 62)
        struct.pack_into("<Q", data, 40, 64)
        struct.pack_into("<HH", data, 58, 64, 5)
        for index, (kind, content, link) in enumerate([
            (3, strings, 0), (11, symbols, 1), (0x6FFFFFFF, versym, 2),
            (0x6FFFFFFE, need, 1),
        ], start=1):
            offset = len(data)
            data.extend(content)
            struct.pack_into("<IIQQQQIIQQ", data, 64 + index * 64,
                             0, kind, 0, 0, offset, len(content), link, 0, 1, 0)
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "copy-object.elf"
            path.write_bytes(data)
            self.assertEqual(generate.elf_import_versions(path), [("libc.so.6", "optind", "GLIBC_2.2.5")])


if __name__ == "__main__":
    unittest.main()
