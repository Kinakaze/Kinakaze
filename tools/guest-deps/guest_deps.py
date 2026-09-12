"""Verified Debian ELF cache; no package installation or maintainer scripts.

Only explicitly locked packages can be fetched. Archives and each materialized
file are checked against SHA-256 before the root preparer may consume them.
"""
from dataclasses import dataclass
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import posixpath
import struct
import tarfile
import urllib.parse
import urllib.request
import uuid

MAX_ARCHIVE = 128 * 1024 * 1024
MAX_EXPANDED = 512 * 1024 * 1024
OFFICIAL_HOSTS = {"deb.debian.org", "security.debian.org", "snapshot.debian.org", "archive.debian.org"}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def relative_path(value):
    if not isinstance(value, str) or "\\" in value or ":" in value or "\0" in value:
        raise ValueError(f"invalid package path: {value!r}")
    return archive_path(value)


def archive_path(value):
    """Validate a POSIX member name used only for in-memory archive lookup.

    Debian uses colons in Perl manuals and literal backslashes in systemd unit
    names. Host materialization must still pass the stricter relative_path.
    """
    if not isinstance(value, str) or "\0" in value:
        raise ValueError(f"invalid archive path: {value!r}")
    path = PurePosixPath(value)
    if path.is_absolute() or ".." in path.parts or not path.parts:
        raise ValueError(f"package path escapes root: {value!r}")
    return path.as_posix()


@dataclass(frozen=True)
class ElfInfo:
    needed: tuple
    soname: str | None
    runpaths: tuple
    interpreter: str | None


def elf_info(data):
    """Read bounded ELF64 x86-64 dynamic metadata, never execute the file."""
    if (len(data) < 64 or data[:7] != b"\x7fELF\x02\x01\x01"
            or struct.unpack_from("<H", data, 18)[0] != 62
            or struct.unpack_from("<H", data, 16)[0] not in (2, 3)):
        raise ValueError("expected x86-64 ELF64 executable/shared object")
    phoff = struct.unpack_from("<Q", data, 32)[0]
    entsize, count = struct.unpack_from("<HH", data, 54)
    if entsize < 56 or phoff + entsize * count > len(data):
        raise ValueError("ELF program headers exceed file")
    loads, dynamic, interpreter = [], None, None
    for index in range(count):
        kind, _, offset, address, _, size, memsize, _ = struct.unpack_from("<IIQQQQQQ", data, phoff + index * entsize)
        if offset + size > len(data) or (kind == 1 and size > memsize):
            raise ValueError("ELF segment exceeds file")
        if kind == 1:
            loads.append((address, offset, size))
        elif kind == 2:
            if dynamic is not None or size % 16:
                raise ValueError("invalid ELF dynamic segment")
            dynamic = offset, size
        elif kind == 3:
            value = data[offset:offset + size]
            if not value.endswith(b"\0") or b"\0" in value[:-1]:
                raise ValueError("invalid ELF interpreter")
            interpreter = value[:-1].decode("utf-8")
    if dynamic is None:
        return ElfInfo((), None, (), interpreter)
    needed, paths, soname, table, table_size = [], [], None, None, None
    terminated = False
    for offset in range(dynamic[0], dynamic[0] + dynamic[1], 16):
        tag, value = struct.unpack_from("<qQ", data, offset)
        if tag == 0:
            terminated = True
            break
        if tag == 1:
            needed.append(value)
        elif tag == 5:
            table = value
        elif tag == 10:
            table_size = value
        elif tag == 14:
            soname = value
        elif tag in (15, 29):
            paths.append(value)
    if not terminated:
        raise ValueError("ELF dynamic table is not terminated")
    if not needed and soname is None and not paths:
        return ElfInfo((), None, (), interpreter)
    if table is None or table_size is None:
        raise ValueError("ELF dynamic string table is missing")
    segment = next(((address, offset, size) for address, offset, size in loads
                    if address <= table and table + table_size <= address + size), None)
    if segment is None:
        raise ValueError("ELF dynamic string table exceeds mapped file bytes")
    offset = segment[1] + table - segment[0]
    strings = data[offset:offset + table_size]

    def string(index):
        if index >= len(strings):
            raise ValueError("ELF dynamic string offset exceeds table")
        end = strings.find(b"\0", index)
        if end < 0:
            raise ValueError("unterminated ELF dynamic string")
        return strings[index:end].decode("utf-8")

    return ElfInfo(tuple(string(index) for index in needed),
                   None if soname is None else string(soname),
                   tuple(part for index in paths for part in string(index).split(":")), interpreter)


def linux_x86_64_member(name, data):
    """JNA jars contain other Unix ELFs; those are not Linux dependencies."""
    foreign = ("freebsd", "openbsd", "netbsd", "dragonfly", "sunos", "solaris", "darwin", "macos", "windows", "android", "/aix/")
    lowered = "/" + name.lower()
    return (len(data) >= 64 and data[:7] == b"\x7fELF\x02\x01\x01"
            and data[7] in (0, 3) and struct.unpack_from("<H", data, 18)[0] == 62
            and not any(token in lowered for token in foreign))


def ar_members(data):
    if not data.startswith(b"!<arch>\n"):
        raise ValueError("Debian package is not an ar archive")
    members, offset = {}, 8
    while offset < len(data):
        if offset + 60 > len(data):
            raise ValueError("truncated ar header")
        header = data[offset:offset + 60]
        if header[58:60] != b"`\n":
            raise ValueError("invalid ar header")
        size = int(header[48:58])
        name = header[:16].decode("ascii").strip().rstrip("/")
        if size < 0 or offset + 60 + size > len(data) or name in members:
            raise ValueError("invalid or duplicated ar member")
        members[name] = data[offset + 60:offset + 60 + size]
        offset += 60 + size + size % 2
    if members.get("debian-binary") != b"2.0\n":
        raise ValueError("unsupported Debian package format")
    return members


def tar_members(blob):
    members, total = {}, 0
    with tarfile.open(fileobj=io.BytesIO(blob), mode="r:*") as archive:
        for member in archive:
            if member.isdir():
                continue
            name = archive_path(member.name)
            if name in members or member.size < 0:
                raise ValueError("duplicate or invalid tar member")
            if member.isfile():
                total += member.size
                if total > MAX_EXPANDED:
                    raise ValueError("expanded package exceeds size limit")
                data = archive.extractfile(member).read()
                if len(data) != member.size:
                    raise ValueError("truncated tar file")
                members[name] = ("file", data)
            elif member.issym() or member.islnk():
                link = member.linkname
                # Absolute Debian links name the guest root. Resolve only in
                # this member map; never create host symlinks or open host paths.
                target = posixpath.normpath(link.lstrip('/') if link.startswith('/') else
                                           posixpath.join(posixpath.dirname(name), link) if member.issym() else link)
                members[name] = ("link", archive_path(target))
            else:
                raise ValueError(f"unsupported package member type: {name}")
    return members


def resolve_member(members, name):
    seen, name = set(), archive_path(name)
    while True:
        if name in seen:
            raise ValueError("package link cycle")
        seen.add(name)
        kind, value = members[name]
        if kind == "file":
            return name, value
        name = value


def checked(data, record, label):
    if len(data) != record["size"] or sha256(data) != record["sha256"]:
        raise ValueError(f"{label}: size/SHA-256 does not match dependency lock")
    return data


def atomic_write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + "." + uuid.uuid4().hex + ".tmp")
    try:
        temporary.write_bytes(data)
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


class PackageCache:
    def __init__(self, lock, directory, offline=False):
        self.lock_path = Path(lock).resolve()
        self.directory = Path(directory).resolve()
        self.offline = offline
        self.lock = json.loads(self.lock_path.read_text(encoding="utf-8"))
        if self.lock.get("schema") != 1 or self.lock.get("guest") != "linux-x86_64":
            raise ValueError("unsupported guest dependency lock")
        self.packages, self.libraries, self.loaded = {}, {}, {}
        for package in self.lock["packages"]:
            if package["architecture"] not in ("amd64", "all"):
                raise ValueError("locked package architecture is not amd64 or all")
            key = package["package"]
            if key in self.packages:
                raise ValueError("duplicate locked package")
            self.packages[key] = package
            for library in package["libraries"]:
                name = library.get("name", library["soname"])
                if relative_path(name) != name or "/" in name:
                    raise ValueError("invalid library lookup filename")
                if name in self.libraries:
                    raise ValueError("duplicate locked SONAME")
                self.libraries[name] = (package, library)

    def _load(self, package):
        key = package["package"]
        if key in self.loaded:
            return self.loaded[key]
        filename = relative_path(package["filename"])
        if "/" in filename or not 0 < package["size"] <= MAX_ARCHIVE:
            raise ValueError("invalid locked package filename/size")
        path = self.directory / filename
        if path.is_file():
            blob = checked(path.read_bytes(), package, filename)
        else:
            if self.offline:
                raise FileNotFoundError(f"offline dependency package missing: {path}; fetch with guest-deps/fetch.py")
            url = package["url"]
            parsed = urllib.parse.urlparse(url)
            if parsed.scheme != "https" or parsed.hostname not in OFFICIAL_HOSTS:
                raise ValueError("dependency download must use an official Debian HTTPS host")
            print(f"Fetching {key} {package['version']} ({package['architecture']})", flush=True)
            with urllib.request.urlopen(url, timeout=45) as response:
                final = urllib.parse.urlparse(response.url)
                if final.scheme != "https" or final.hostname not in OFFICIAL_HOSTS:
                    raise ValueError("dependency redirect left official Debian HTTPS hosts")
                blob = checked(response.read(package["size"] + 1), package, filename)
            atomic_write(path, blob)
        archive = ar_members(blob)
        controls = [value for name, value in archive.items() if name.startswith("control.tar.")]
        payloads = [value for name, value in archive.items() if name.startswith("data.tar.")]
        if len(controls) != 1 or len(payloads) != 1:
            raise ValueError("Debian package must have one control and one data archive")
        _, control = resolve_member(tar_members(controls[0]), "control")
        fields = dict(line.split(": ", 1) for line in control.decode("utf-8").splitlines()
                      if line and not line[0].isspace() and ": " in line)
        for field, expected in (("Package", key), ("Version", package["version"]), ("Architecture", package["architecture"])):
            if fields.get(field) != expected:
                raise ValueError(f"Debian control {field} disagrees with lock")
        source_field = fields.get("Source", key)
        source, separator, source_version = source_field.partition(" (")
        if separator:
            if not source_version.endswith(")"):
                raise ValueError("invalid Debian source version field")
            source_version = source_version[:-1]
        else:
            source_version = fields["Version"]
        if source != package["source_package"] or source_version != package["source_version"]:
            raise ValueError("Debian source package/version disagrees with lock")
        members = tar_members(payloads[0])
        records = package["libraries"] + package.get("documents", []) + package.get("files", [])
        # Tree contents are authenticated by the locked package digest. Per-file
        # hashes also validate existing cache entries. No paths are extracted.
        for tree in package.get("trees", []):
            prefix = relative_path(tree["member"]) + "/"
            destination = relative_path(tree["install_path"]) + "/"
            excluded = {archive_path(name) for name in tree.get("exclude", [])}
            if any(not name.startswith(prefix) or name not in members for name in excluded):
                raise ValueError("invalid locked tree exclusion")
            selected = sorted(name for name in members if name.startswith(prefix) and name not in excluded)
            if not selected:
                raise ValueError(f"locked package tree is empty: {key}: {prefix}")
            for name in selected:
                resolved, content = resolve_member(members, name)
                records.append({"member": name, "resolved_member": resolved,
                                "install_path": destination + name[len(prefix):],
                                "size": len(content), "sha256": sha256(content)})
        files = {}
        for record in records:
            resolved, content = resolve_member(members, record["member"])
            if resolved != record.get("resolved_member", record["member"]):
                raise ValueError("Debian member/link target disagrees with lock")
            checked(content, record, record["member"])
            if "soname" in record and elf_info(content).soname != record["soname"]:
                raise ValueError("ELF SONAME disagrees with lock")
            install = relative_path(record["install_path"])
            if install in files:
                raise ValueError(f"duplicate locked install path: {key}: {install}")
            output = self.directory / "files" / package["sha256"] / install
            if not output.is_file() or sha256(output.read_bytes()) != record["sha256"]:
                atomic_write(output, content)
            files[install] = output
        self.loaded[key] = files
        return files

    def resolve(self, soname):
        match = self.libraries.get(soname)
        if match is None:
            return None
        package, library = match
        files = self._load(package)
        provenance = {key: package[key] for key in ("package", "version", "architecture", "url", "sha256", "source_package", "source_version")}
        documents = [(record["install_path"], files[record["install_path"]]) for record in package.get("documents", [])]
        return files[library["install_path"]], library["install_path"], provenance, documents

    def materialize(self, name):
        """Return only reviewed files from a named package, with provenance."""
        package = self.packages.get(name)
        if package is None:
            raise ValueError(f"package is absent from dependency lock: {name}")
        files = self._load(package)
        provenance = {key: package[key] for key in ("package", "version", "architecture", "url", "sha256", "source_package", "source_version")}
        return files, provenance

    def fetch_all(self):
        for package in self.packages.values():
            self._load(package)
