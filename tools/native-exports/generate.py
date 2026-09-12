#!/usr/bin/env python3
"""Update native linker exports and object queries from compiled definitions.

The checked .def inputs let ordinary Cargo builds link without running this tool.
Regeneration requires real PE definitions: macro expansions and cfg selection
are decided by rustc, never guessed here. No module inventory is written.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import zipfile

import evidence as abi_evidence
from build_inputs import load_inputs, source_directory, read_cargo, soname

ROOT = Path(__file__).resolve().parents[2]
INPUTS = load_inputs(ROOT)

# The guest-visible payload size is intentionally NOT sizeof the Rust wrapper:
# CopiedInt/Pointer contain private relocation bookkeeping after this payload.
# In6Addr has repr(C, align(4)), matching the Linux in6_addr union alignment.
OBJECT_LAYOUTS = {
    "__libc_stack_end": (8, 8),
    "_libc_intl_domainname": (5, 1),
    "argp_err_exit_status": (4, 4),
    "error_message_count": (4, 4), "error_one_per_line": (4, 4),
    "error_print_progname": (8, 8),
    "argp_program_bug_address": (8, 8),
    "argp_program_version": (8, 8),
    "argp_program_version_hook": (8, 8),
    "timezone": (8, 8), "__timezone": (8, 8),
    "tzname": (16, 8), "__tzname": (16, 8),
    "daylight": (4, 4), "__daylight": (4, 4),
    "_environ": (8, 8),
    "__libc_single_threaded": (1, 1),
    "optind": (4, 4), "optopt": (4, 4), "opterr": (4, 4),
    "optarg": (8, 8), "environ": (8, 8), "__environ": (8, 8),
    "stdin": (8, 8), "stdout": (8, 8), "stderr": (8, 8),
    "program_invocation_name": (8, 8),
    "program_invocation_short_name": (8, 8),
    "__progname": (8, 8), "__progname_full": (8, 8),
    "in6addr_any": (16, 4), "in6addr_loopback": (16, 4),
    "re_syntax_options": (8, 8), "_Xglobal_lock": (8, 8),
    "_XLockMutex_fn": (8, 8), "_XUnlockMutex_fn": (8, 8),
    "obstack_alloc_failed_handler": (8, 8), "obstack_exit_failure": (4, 4),
    "signgam": (4, 4), "__signgam": (4, 4),
    "xcb_big_requests_id": (16, 8),
}

# Before glibc merged libpthread into libc, these public ABI entries were also
# exported by libpthread. Retain that observed compatibility surface while
# forwarding into the same runtime implementation and storage as libc.
PTHREAD_LIBC_FORWARDERS = {
    "pthread_mutexattr_setpshared",
    "__res_state", "fork", "pause", "sigwait", "tcdrain",
    "pthread_rwlockattr_init", "pthread_rwlockattr_destroy", "pthread_attr_setscope",
    "__h_errno_location", "__libc_current_sigrtmax", "__libc_current_sigrtmin",
    "longjmp", "siglongjmp", "pthread_getcpuclockid",
    "__errno_location", "__pthread_key_create", "accept", "close", "connect",
    "fcntl", "flockfile", "funlockfile", "lseek", "nanosleep", "open", "open64",
    "raise", "read", "recv", "recvfrom", "send", "sendto", "write",
    "tss_create", "tss_delete", "tss_get", "tss_set",
    "fsync", "lseek64", "msync", "pread64", "pwrite64", "recvmsg", "sendmsg",
    "pthread_getaffinity_np", "pthread_setaffinity_np", "sigaction", "waitpid",
    "sem_destroy", "sem_init", "sem_post", "sem_timedwait", "sem_trywait", "sem_wait",
}

# The same signal-mask implementation is also imported from modern libc.
LIBC_PTHREAD_FORWARDERS = {"pthread_attr_getschedpolicy", "pthread_attr_getschedparam", "pthread_attr_getinheritsched", "pthread_attr_setschedparam", "pthread_attr_setschedpolicy", "pthread_attr_setinheritsched",
                          "__pthread_register_cancel", "__pthread_unregister_cancel", "__pthread_unwind_next",
                          "__pthread_register_cancel_defer", "__pthread_unregister_cancel_restore",
                          "pthread_barrier_init", "pthread_barrier_wait", "pthread_barrier_destroy",
                          "pthread_sigmask", "pthread_mutex_clocklock", "pthread_cond_clockwait",
                          "pthread_mutexattr_setrobust", "pthread_mutexattr_getrobust", "pthread_mutex_consistent",
                          "pthread_mutexattr_setprotocol", "pthread_mutexattr_getprotocol",
                          "pthread_attr_setaffinity_np", "pthread_attr_getaffinity_np",
                          "pthread_setschedparam", "pthread_getschedparam", "pthread_setschedprio"}
LIBC_RT_FORWARDERS = {"timer_create", "timer_delete", "timer_settime", "timer_gettime", "timer_getoverrun"}
LIBC_LIBM_FORWARDERS = {"copysign", "__isinf", "__isnan", "__isnanf", "isinf", "isnan", "isinff", "isnanf"}

# These entries belong to the one process linker. Alias both public ABIs to
# the compiled functions; a Rust call wrapper loses RTLD_NEXT's return address.
LOADER_EXPORTS = {"dlopen", "dlsym", "dlvsym", "dlclose", "dlerror",
                  "dladdr", "dlinfo", "dl_iterate_phdr"}


def add_loader_exports(exports):
    for name, entry in exports["ld-linux-x86-64"].items():
        if name in exports["libc"]:
            raise ValueError(f"Duplicate libc implementation of loader entry {name}")
        exports["libc"][name] = dict(entry)


def add_compatibility_exports(exports):
    # ABI synonyms bind the same implementation directly, with no Rust thunk.
    for alias, original in {"_Exit": "_exit", "__isoc99_vfscanf": "vfscanf",
                            "__isoc99_scanf": "scanf", "__isoc99_vscanf": "vscanf",
                            "__isoc99_vsscanf": "vsscanf", "clearerr_unlocked": "clearerr",
                            "__pread64_chk": "__pread_chk", "__stpcpy": "stpcpy", "__mempcpy": "mempcpy",
                            "__dcgettext": "dcgettext", "strtod_l": "__strtod_l",
                            "__strndup": "strndup", "strftime_l": "__strftime_l",
                            "strtof_l": "__strtof_l", "ftw64": "ftw", "nftw64": "nftw",
                            "versionsort64": "versionsort", "fgetpos64": "fgetpos", "fsetpos64": "fsetpos",
                            "__fread_unlocked_chk": "__fread_chk"}.items():
        if original not in exports["libc"]:
            raise ValueError(f"Compatibility export libc:{alias} has no libc implementation {original}")
        exports["libc"][alias] = dict(exports["libc"][original], name=alias)
    # Linux AMD64 long and long long have the same size and return ABI.
    for alias, original in {"llrint": "lrint", "llrintf": "lrintf",
                            "drem": "remainder", "gamma": "lgamma",
                            "__atan2_finite": "atan2", "__exp_finite": "exp", "__fpclassifyf": "fpclassifyf", "__fpclassify": "fpclassify"}.items():
        if original not in exports["libm"]:
            raise ValueError(f"Compatibility export libm:{alias} has no libm implementation {original}")
        exports["libm"][alias] = dict(exports["libm"][original], name=alias)
    for destination, source, names in (("libpthread", "libc", PTHREAD_LIBC_FORWARDERS),
                                       ("libc", "libpthread", LIBC_PTHREAD_FORWARDERS),
                                       ("libc", "libm", LIBC_LIBM_FORWARDERS),
                                       ("libc", "librt", LIBC_RT_FORWARDERS),
                                       ("ld-linux-x86-64", "libc", {"__tls_get_addr", "__libc_stack_end"})):
        for name in sorted(names):
            if name not in exports[source]:
                raise ValueError(f"Compatibility export {destination}:{name} has no {source} implementation")
            exports[destination].setdefault(name, dict(exports[source][name]))


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def native_symbols(path: Path) -> dict[str, str]:
    """Read actual PE definitions and classify their containing section."""
    data = path.read_bytes()
    pe = struct.unpack_from('<I', data, 0x3c)[0]
    if data[:2] != b'MZ' or data[pe:pe+4] != b'PE\0\0':
        raise ValueError(f'Not a native PE image: {path}')
    count, optional_size = struct.unpack_from('<H', data, pe+6)[0], struct.unpack_from('<H', data, pe+20)[0]
    optional = pe + 24
    if struct.unpack_from('<H', data, optional)[0] != 0x20b:
        raise ValueError(f'Not a PE32+ image: {path}')
    sections = []
    for index in range(count):
        at = optional + optional_size + index * 40
        virtual_size, rva, raw_size, raw = struct.unpack_from('<IIII', data, at+8)
        flags = struct.unpack_from('<I', data, at+36)[0]
        sections.append((rva, max(virtual_size,raw_size), raw, raw_size, flags))
    def offset(rva, size=1):
        for start, _, raw, raw_size, _ in sections:
            if start <= rva and rva-start+size <= raw_size and raw+rva-start+size <= len(data):
                return raw+rva-start
        raise ValueError(f'Unbacked RVA in {path}: {rva:x}')
    export_rva, export_size = struct.unpack_from('<II', data, optional+112)
    directory = offset(export_rva, 40)
    functions, names, addresses, pointers, ordinals = struct.unpack_from('<IIIII', data, directory+20)
    if max(functions,names) > 65536:
        raise ValueError(f'Excessive export count in {path}')
    address_table, name_table, ordinal_table = offset(addresses, functions*4), offset(pointers,names*4), offset(ordinals,names*2)
    result = {}
    for index in range(names):
        string = offset(struct.unpack_from('<I', data, name_table+index*4)[0])
        end = data.index(b'\0',string,string+4097)
        name = data[string:end].decode('utf-8')
        if '@' in name: continue
        ordinal = struct.unpack_from('<H',data,ordinal_table+index*2)[0]
        if ordinal >= functions: raise ValueError(f'Invalid export ordinal in {path}')
        address = struct.unpack_from('<I',data,address_table+ordinal*4)[0]
        if export_rva <= address < export_rva+export_size:
            continue  # A forwarder is not a definition owned by this image.
        flags = next(flags for start,size,_,_,flags in sections if start <= address < start+size)
        result[name] = 'T' if flags & 0x20000000 else 'D'
    return result


def libc_aliases() -> list[tuple[str, str]]:
    """Preserve alternate spellings from libc's standard linker input."""
    definition = (ROOT / 'libs/libc/exports.def').read_text(encoding='utf-8')
    return [(name, target) for name, target in re.findall(
        r'^\s*"([A-Za-z_][A-Za-z_0-9]*)"=(kinakaze_abi_[A-Za-z_0-9]+)(?:\s|$)',
        definition, re.MULTILINE) if name != target.removeprefix('kinakaze_abi_')]


def validate_literal_exports(provider: str, definitions: dict[str, str]):
    """Reject a stale top-level archive after only cargo test rebuilt deps/.

    Compiled symbols remain authoritative for macros. Literal export attributes
    provide an additional drift check for newly added public entry points.
    """
    prefix = f"kinakaze_engine_{provider}_"
    pattern = re.compile(r'^\s*#\[unsafe\(export_name\s*=\s*"('
                         + re.escape(prefix) + r'[^"\n]+)"\)\]', re.MULTILINE)
    for path in (source_directory(ROOT, provider) / "src").rglob("*.rs"):
        source = path.read_text(encoding="utf-8")
        targets = pattern.findall(source)
        if provider == "libc":
            # global_asm symbols need an explicit linker export: unlike Rust
            # no_mangle functions, .globl alone does not publish a PE entry.
            targets.extend(re.findall(r'^\.globl (kinakaze_abi_\w+)\s*$', source, re.MULTILINE))
        for target in targets:
            if target not in definitions:
                raise ValueError(f"Source export {target} is missing from {provider}'s archive; "
                                 "run cargo build --lib, not only cargo test")


def guest_exports(provider: str, definitions: dict[str, str], aliases=()):
    """Filter actual external exports, retaining data classification and aliases."""
    result = {}
    if provider == "libc":
        pairs = [(name.removeprefix("kinakaze_abi_"), name)
                 for name in definitions if name.startswith("kinakaze_abi_")]
        pairs.extend(aliases)
        # GNU regex storage is the one original unprefixed libc guest object.
        for name in ("re_syntax_options", "argp_err_exit_status", "argp_program_bug_address", "argp_program_version", "argp_program_version_hook"):
            if name in definitions:
                pairs.append((name, name))
    elif provider == "ld-linux-x86-64":
        pairs = [(name, f"kinakaze_process_{name}") for name in sorted(LOADER_EXPORTS)]
    elif provider == "libpthread":
        pairs = [(name, name) for name in definitions
                 if name.startswith(("pthread_", "__pthread_")) or name == "sched_yield"]
    else:
        prefix = f"kinakaze_engine_{provider}_"
        pairs = [(name.removeprefix(prefix), name) for name in definitions
                 if name.startswith(prefix)]
        if provider == "libm" and "kinakaze_engine_libm_signgam" in definitions:
            pairs.append(("__signgam", "kinakaze_engine_libm_signgam"))
    for guest, target in pairs:
        if target not in definitions:
            raise ValueError(f"Source alias {guest} references uncompiled symbol {target}")
        letter = definitions[target]
        if letter == "T":
            entry = dict(name=guest, runtime_export=target, kind="function", size=0, alignment=1)
        elif letter in {"B", "D", "R", "S"}:
            if guest not in OBJECT_LAYOUTS:
                raise ValueError(f"Object {provider}:{guest} needs an explicit guest ABI layout")
            size, alignment = OBJECT_LAYOUTS[guest]
            entry = dict(name=guest, runtime_export=target, kind="object", size=size, alignment=alignment)
        else:
            raise ValueError(f"Unsupported definition kind {letter} for {target}")
        if guest in result and result[guest] != entry:
            raise ValueError(f"Conflicting guest symbol {guest}")
        result[guest] = entry
    return result


def belongs(frontend: str, guest: str) -> bool:
    if frontend == "ld-linux-x86-64":
        return guest in LOADER_EXPORTS | {"__tls_get_addr", "__libc_stack_end"}
    if frontend == "libdl":
        return guest in LOADER_EXPORTS
    if frontend == "libGL":
        return guest.startswith("gl")
    if frontend == "libGLX":
        return guest.startswith("glX")
    if frontend == "libEGL":
        return guest.startswith("egl")
    if frontend in {"libOpenGL", "libGLESv2"}:
        return guest.startswith("gl") and not guest.startswith("glX")
    xfamilies = {
        "libXrandr": ("XRR",), "libXcursor": ("Xcursor",), "libXi": ("XI",),
        "libXinerama": ("Xinerama",), "libXrender": ("XRender",),
        "libXxf86vm": ("XF86VidMode",),
        "libX11-xcb": ("XGetXCBConnection", "XSetEventQueueOwner"),
        "libXext": ("XShm", "XShape", "Xdbe", "XSync", "DPMS", "Xext"),
    }
    if frontend in xfamilies:
        if frontend == "libXi":
            if guest in {"XOpenDevice", "XCloseDevice", "XSetDeviceMode", "XSetDeviceButtonMapping",
                         "XListInputDevices", "XFreeDeviceList", "XGrabDevice", "XUngrabDevice",
                         "XSelectExtensionEvent", "XGetDeviceMotionEvents", "XFreeDeviceMotionEvents",
                         "XQueryDeviceState", "XFreeDeviceState", "XGetDeviceProperty"}:
                return True
            # XIQueryVersion is XInput; XInitThreads, XInternAtom, XIfEvent,
            # XIconifyWindow, XIntersectRegion and XIMOfIC are core Xlib.
            return guest != "XIMOfIC" and guest.startswith("XI") and len(guest) > 2 and guest[2].isupper()
        if frontend == "libXext" and guest in {"XSync", "XSynchronize"}:
            return False
        return guest.startswith(xfamilies[frontend])
    if frontend == "libX11":
        return not any(belongs(family, guest) for family in xfamilies)
    if frontend == "libpulse-simple":
        return guest.startswith("pa_simple_")
    if frontend == "libpulse":
        return not guest.startswith("pa_simple_")
    return True


def elf_import_versions(path: Path):
    """Read ELF64 versym/verneed, including versioned COPY-relocation objects."""
    return elf_bytes_import_versions(path.read_bytes(), str(path))


def elf_bytes_import_versions(data: bytes, label: str):
    if data[:6] != b"\x7fELF\x02\x01" or len(data) < 64 or data[7] not in (0, 3) or struct.unpack_from("<H", data, 18)[0] != 62:
        return []
    shoff = struct.unpack_from("<Q", data, 40)[0]
    shentsize, shnum = struct.unpack_from("<HH", data, 58)
    if shentsize != 64 or not shnum or shoff + shnum * 64 > len(data):
        return []
    sections = [struct.unpack_from("<IIQQQQIIQQ", data, shoff + 64 * i)
                for i in range(shnum)]
    def payload(section):
        offset, size = section[4:6]
        if offset + size > len(data):
            raise ValueError(f"Out-of-bounds ELF section in {label}")
        return data[offset:offset + size]
    def string(table, offset):
        end = table.index(b"\0", offset)
        return table[offset:end].decode("utf-8")
    versions = {}
    for section in sections:
        if section[1] != 0x6FFFFFFE:
            continue
        strings = payload(sections[section[6]])
        needs = payload(section)
        pos = 0
        while pos < len(needs):
            _, count, library, aux, following = struct.unpack_from("<HHIII", needs, pos)
            soname = string(strings, library)
            cursor = pos + aux
            for _ in range(count):
                _, _, index, name, next_aux = struct.unpack_from("<IHHII", needs, cursor)
                versions[index & 0x7FFF] = (soname, string(strings, name))
                cursor += next_aux
            if not following:
                break
            pos += following
    result = []
    for section in sections:
        if section[1] != 0x6FFFFFFF:
            continue
        dynsym = sections[section[6]]
        strings = payload(sections[dynsym[6]])
        syms, vers = payload(dynsym), payload(section)
        for index in range(min(len(syms) // 24, len(vers) // 2)):
            version = struct.unpack_from("<H", vers, index * 2)[0] & 0x7FFF
            if version in versions:
                offset, _, _, _, _, _ = struct.unpack_from("<IBBHQQ", syms, index * 24)
                soname, name = versions[version]
                result.append((soname, string(strings, offset), name))
    return result


def observation_files(paths):
    """Expand explicitly requested directories; archive members stay in memory."""
    result = set()
    for value in paths:
        path = Path(value).resolve()
        if path.is_dir():
            result.update(candidate for candidate in path.rglob("*") if candidate.is_file())
        elif path.is_file():
            result.add(path)
        else:
            raise ValueError(f"Missing observation input: {path}")
    return sorted(result)


def observed_elfs(paths):
    for path in observation_files(paths):
        try:
            display = path.relative_to(ROOT).as_posix()
        except ValueError:
            display = path.name
        with path.open("rb") as stream:
            magic = stream.read(6)
        if magic[:4] == b"PK\x03\x04" and path.suffix.lower() in {".zip", ".jar"}:
            archive_digest = None
            with zipfile.ZipFile(path) as archive:
                for member in sorted(archive.infolist(), key=lambda entry: entry.filename):
                    if member.is_dir():
                        continue
                    # Multi-platform JNI archives also carry BSD/Solaris ELF
                    # binaries. Some identify as System V OSABI 0, so retain
                    # their explicit package-platform distinction as well.
                    if any(part.lower().startswith(("freebsd", "openbsd", "netbsd", "dragonflybsd", "sunos", "solaris", "android"))
                           for part in Path(member.filename).parts):
                        continue
                    with archive.open(member) as stream:
                        prefix = stream.read(20)
                        if prefix[:6] != b"\x7fELF\x02\x01" or len(prefix) < 20 or prefix[7] not in (0, 3) or struct.unpack_from("<H", prefix, 18)[0] != 62:
                            continue
                        data = prefix + stream.read()
                    if archive_digest is None:
                        archive_digest = digest(path)
                    yield data, dict(path=display, member=member.filename,
                                     archive_sha256=archive_digest,
                                     sha256=hashlib.sha256(data).hexdigest())
        elif magic == b"\x7fELF\x02\x01":
            data = path.read_bytes()
            if len(data) >= 20 and data[7] in (0, 3) and struct.unpack_from("<H", data, 18)[0] == 62:
                yield data, dict(path=display, sha256=hashlib.sha256(data).hexdigest())


def import_inventory(paths):
    entries, provenance = {}, []
    for data, evidence in observed_elfs(paths):
        rows = elf_bytes_import_versions(data, evidence["path"] + "!" + evidence.get("member", ""))
        if not rows:
            continue
        provenance.append(evidence)
        for soname, name, version in rows:
            entries.setdefault((soname, name), set()).add(version)
    return entries, provenance


def default_observation_paths():
    guest_root = ROOT / "artifacts/guest-root"
    paths = [guest_root / "usr/bin/busybox", guest_root / "usr/bin/curl"]
    paths += list((guest_root / "usr/lib").glob("*.so*"))
    paths += list((guest_root / "usr/lib/x86_64-linux-gnu").glob("*.so*"))
    paths += list((guest_root / "usr/lib/jvm").glob("*/bin/java"))
    paths += list((guest_root / "usr/lib/jvm").glob("*/lib/**/*.so"))
    # Some JNI artifacts (for example JNA) contain Linux ELF members without
    # "natives-linux" in the JAR filename. Inspect every installed library JAR.
    paths += list((guest_root / "minecraft/libraries").glob("**/*.jar"))
    paths += list((guest_root / "tests").glob("**/*natives-linux.jar"))
    return [path for path in paths if path.is_file()]


def module_outputs(module, native_owners):
    """Emit ordinary linker exports and the data-size query C ABI."""
    definition = [f'LIBRARY "{module["soname"]}"', 'EXPORTS']
    objects = []
    for symbol in module['exports']:
        target = symbol['runtime_export']
        owner = native_owners.get(target, module['soname'])
        binding = target if owner == module['soname'] else f'{owner}.{target}'
        suffix = ' DATA' if symbol['kind'] == 'object' else ''
        if owner == module['soname']:
            definition.append(f'  {target}{suffix}')
        names = [symbol['name'], *(f'{symbol["name"]}@{version}' for version in symbol['versions'])]
        for name in names:
            private = ' PRIVATE' if name != target or owner != module['soname'] else ''
            definition.append(f'  "{name}"={binding}{suffix}{private}')
        if symbol['kind'] == 'object':
            objects.append(f'        b"{symbol["name"]}" => ({symbol["alignment"]}u64 << 32) | {symbol["size"]},')
    source = [
        '// Generated object layout query; PE export tables do not carry data sizes.',
        '// Buffers are borrowed only for this call. No memory ownership crosses the ABI.',
        '#[rustfmt::skip]',
        '#[unsafe(no_mangle)]',
        'pub unsafe extern "C" fn kinakaze_module_object_v1(name: *const u8, length: usize) -> u64 {',
        '    if name.is_null() || length > 128 { return 0; }',
        '    match unsafe { core::slice::from_raw_parts(name, length) } {',
        *objects,
        '        _ => 0,',
        '    }',
        '}',
    ]
    directory = module['directory']
    return {directory / 'exports.def': '\n'.join(definition) + '\n',
            directory / 'src/object_layout.rs': '\n'.join(source) + '\n'}


def version_evidence(args):
    """Normal builds use checked ABI evidence, without a developer guest root."""
    if args.observe or args.refresh_evidence:
        inventory, evidence = import_inventory(args.observe if args.observe else default_observation_paths())
        if not inventory or not evidence:
            raise ValueError("No versioned ELF evidence found; prepare artifacts/guest-root or supply --observe")
        return inventory, evidence
    inventory, evidence = abi_evidence.read(ROOT)
    if paths := getattr(args, "extend_evidence", None):
        additional, sources = import_inventory(paths)
        if not additional or not sources:
            raise ValueError("No versioned ELF evidence found in extension inputs")
        for key, versions in additional.items():
            inventory.setdefault(key, set()).update(versions)
        known = {json.dumps(entry, sort_keys=True) for entry in evidence}
        for source in sources:
            key = json.dumps(source, sort_keys=True)
            if key not in known:
                evidence.append(source)
                known.add(key)
    return inventory, evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image-dir", type=Path, default=ROOT / "target/debug")
    evidence_group = parser.add_mutually_exclusive_group()
    evidence_group.add_argument("--observe", type=Path, action="append", help="Refresh using these ELF files (repeatable)")
    evidence_group.add_argument("--refresh-evidence", action="store_true", help="Refresh ABI versions from artifacts/guest-root")
    evidence_group.add_argument("--extend-evidence", type=Path, action="append",
                                help="Add observed ABI versions while preserving checked evidence (repeatable)")
    parser.add_argument("--check", action="store_true", help="Validate generated files without changing them")
    args = parser.parse_args()
    observed, evidence = version_evidence(args)
    definitions, exports = {}, {}
    for provider in sorted({module['implementation'] for name, module in INPUTS.items() if name != 'runtime'}):
        directory = source_directory(ROOT, provider)
        library = read_cargo(directory)["lib"]["name"]
        archive = args.image_dir / f"{library}.dll"
        definitions[provider] = native_symbols(archive)
        validate_literal_exports(provider, definitions[provider])
        aliases = libc_aliases() if provider == "libc" else ()
        exports[provider] = guest_exports(provider, definitions[provider], aliases)
    add_loader_exports(exports)
    add_compatibility_exports(exports)
    x11_frontends = [name for name, module in INPUTS.items() if module['implementation'] == 'libX11']
    for name in exports["libX11"]:
        owners = [frontend for frontend in x11_frontends if belongs(frontend, name)]
        if len(owners) != 1:
            raise ValueError(f"Xlib export {name} must have exactly one module owner; found {owners}")
    runtime = dict(INPUTS['runtime'], exports=[dict(
        name="kinakaze_runtime_abi_version", runtime_export="kinakaze_runtime_abi_version_sysv",
        kind="function", size=0, alignment=1, versions=[], default_version=None)])
    modules = [runtime]
    for frontend, metadata in INPUTS.items():
        if frontend == 'runtime':
            continue
        provider = metadata['implementation']
        symbols = []
        for name, raw in sorted(exports[provider].items()):
            if not belongs(frontend, name):
                continue
            symbol = dict(raw)
            versions = sorted(observed.get((metadata['soname'], name), set()),
                              key=lambda value: [int(piece) if piece.isdigit() else piece
                                                 for piece in re.split(r"(\d+)", value)])
            symbol.update(versions=versions, default_version=versions[-1] if versions else None)
            symbols.append(symbol)
        if not symbols:
            raise ValueError(f"{metadata['soname']} has no compiled exports")
        modules.append(dict(metadata, exports=symbols))
    native_owners = {}
    targets = {symbol['runtime_export'] for module in modules for symbol in module['exports']}
    for provider, compiled in definitions.items():
        owner = soname(source_directory(ROOT, provider))
        for symbol in compiled:
            if symbol in targets and symbol.startswith('kinakaze_'):
                if symbol in native_owners and native_owners[symbol] != owner:
                    raise ValueError(f'Multiple compiled owners for {symbol}')
                native_owners[symbol] = owner
    for module in modules[1:]:
        for symbol in module["exports"]:
            target = symbol['runtime_export']
            if target not in native_owners:
                # Unprefixed pthread definitions also have one compiled owner.
                owners = [provider for provider, compiled in definitions.items() if target in compiled]
                if module['implementation'] in owners:
                    owners = [module['implementation']]
                if len(owners) != 1:
                    raise ValueError(f"No unique compiled owner for {target}: {owners}")
                native_owners[target] = soname(source_directory(ROOT, owners[0]))
    outputs = {}
    outputs.update(abi_evidence.outputs(ROOT, observed, evidence))
    for module in modules:
        outputs.update(module_outputs(module, native_owners))
    for path, contents in outputs.items():
        if args.check:
            if not path.exists() or path.read_text(encoding="utf-8") != contents:
                raise ValueError(f"Stale generated exports: {path}; regenerate after building libraries")
        else:
            path.write_text(contents, encoding="utf-8", newline="\n")
    print(f"{'Checked' if args.check else 'Updated'} {len(modules)} linker inputs, {sum(len(m['exports']) for m in modules)} guest exports")
    print(f"Version evidence: {len(evidence)} ELF files")


if __name__ == "__main__":
    main()
