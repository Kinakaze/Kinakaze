"""Clone a disposable guest root while preserving NTFS case and inode metadata."""

import argparse
import csv
import ctypes
from ctypes import wintypes
import os
from pathlib import Path
import subprocess


class IoStatus(ctypes.Structure):
    _fields_ = (("status", ctypes.c_size_t), ("information", ctypes.c_size_t))


def copy_inode_metadata(source, target):
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    create = kernel.CreateFileW
    create.argtypes = (
        wintypes.LPCWSTR,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.c_void_p,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.HANDLE,
    )
    create.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
    nt = ctypes.WinDLL("ntdll")
    query = nt.NtQueryEaFile
    query.argtypes = (
        wintypes.HANDLE,
        ctypes.POINTER(IoStatus),
        ctypes.c_void_p,
        wintypes.ULONG,
        ctypes.c_ubyte,
        ctypes.c_void_p,
        wintypes.ULONG,
        ctypes.c_void_p,
        ctypes.c_ubyte,
    )
    query.restype = ctypes.c_long
    write = nt.NtSetEaFile
    write.argtypes = (
        wintypes.HANDLE,
        ctypes.POINTER(IoStatus),
        ctypes.c_void_p,
        wintypes.ULONG,
    )
    write.restype = ctypes.c_long
    first = create(str(source), 8, 7, None, 3, 0x02000000, None)
    if first == wintypes.HANDLE(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        buffer, status = ctypes.create_string_buffer(65536), IoStatus()
        code = query(
            first, ctypes.byref(status), buffer, len(buffer), 0, None, 0, None, 1
        )
        if code & 0xFFFFFFFF in (0xC0000052, 0x80000012):
            return
        if code != 0:
            raise OSError(f"NtQueryEaFile failed: {code & 0xFFFFFFFF:#x}: {source}")
    finally:
        kernel.CloseHandle(first)
    if not status.information:
        return
    second = create(str(target), 16, 7, None, 3, 0x02000000, None)
    if second == wintypes.HANDLE(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        length = status.information
        code = write(second, ctypes.byref(status), buffer, length)
        if code != 0:
            raise OSError(f"NtSetEaFile failed: {code & 0xFFFFFFFF:#x}: {target}")
    finally:
        kernel.CloseHandle(second)


def case_sensitive(path):
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    create = kernel.CreateFileW
    create.argtypes = (
        wintypes.LPCWSTR,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.c_void_p,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.HANDLE,
    )
    create.restype = wintypes.HANDLE
    set_info = kernel.SetFileInformationByHandle
    set_info.argtypes = (
        wintypes.HANDLE,
        ctypes.c_int,
        ctypes.c_void_p,
        wintypes.DWORD,
    )
    set_info.restype = wintypes.BOOL
    get_info = kernel.GetFileInformationByHandleEx
    get_info.argtypes = (
        wintypes.HANDLE,
        ctypes.c_int,
        ctypes.c_void_p,
        wintypes.DWORD,
    )
    get_info.restype = wintypes.BOOL
    kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
    handle = create(str(path), 0x180, 7, None, 3, 0x02000000, None)
    if handle == wintypes.HANDLE(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        current = wintypes.DWORD()
        if not get_info(handle, 23, ctypes.byref(current), ctypes.sizeof(current)):
            raise ctypes.WinError(ctypes.get_last_error())
        if current.value & 1:
            return
        flags = wintypes.DWORD(1)
        if not set_info(handle, 23, ctypes.byref(flags), ctypes.sizeof(flags)):
            raise ctypes.WinError(ctypes.get_last_error())
    finally:
        kernel.CloseHandle(handle)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument(
        "--restore-missing",
        action="store_true",
        help="enable case on an existing disposable clone and restore absent source files",
    )
    args = parser.parse_args()
    source, root = args.source.resolve(), args.root.resolve()
    artifacts = Path(__file__).resolve().parents[1] / "artifacts"
    if os.name != "nt" or not source.is_dir():
        parser.error("a Windows source root is required")
    if (
        not root.is_relative_to(artifacts)
        or source == root
        or source.is_relative_to(root)
        or root.is_relative_to(source)
    ):
        parser.error("destination must be a distinct disposable root under artifacts")
    if root.exists() and not args.restore_missing:
        parser.error("destination exists; choose a fresh root")
    root.mkdir(parents=True, exist_ok=True)
    # Match crates/rootfs/src/permissions.rs: enabling case also needs
    # DELETE_CHILD, which an inherited Modify ACL may omit. Grant only that
    # directory right to this task's Windows identity on the disposable clone.
    identity = subprocess.check_output(
        ["whoami", "/user", "/fo", "csv", "/nh"], text=True
    )
    sid = next(csv.reader(identity.splitlines()))[1]
    subprocess.run(
        ["icacls", str(root), "/grant", f"*{sid}:(CI)(DC)"],
        check=True,
        stdout=subprocess.DEVNULL,
    )
    case_sensitive(root)
    # Create and mark directories before copying data; a case-insensitive copy
    # otherwise merges HEAD/head and Perl's Pod/pod even if fixed afterwards.
    if args.restore_missing:
        for current, directories, _ in os.walk(root):
            if Path(current).relative_to(root) in (Path("tmp"), Path("var/tmp")):
                directories[:] = []
            case_sensitive(Path(current))
    count = files = 0
    copy = ctypes.WinDLL("kernel32", use_last_error=True).CopyFileW
    copy.argtypes = (wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.BOOL)
    copy.restype = wintypes.BOOL
    for current, directories, filenames in os.walk(source):
        relative = Path(current).relative_to(source)
        # Previous test workspaces are neither software nor installation state.
        if relative in (Path("tmp"), Path("var/tmp")):
            directories[:] = []
            filenames = []
        target = root / Path(current).relative_to(source)
        target.mkdir(parents=True, exist_ok=True)
        case_sensitive(target)
        copy_inode_metadata(Path(current), target)
        count += 1
        for name in filenames:
            original, destination = Path(current) / name, target / name
            if args.restore_missing and destination.exists():
                continue
            # Copy each exact spelling: robocopy itself also folds names even
            # when both source and destination directories are case-sensitive.
            if not copy(str(original), str(destination), True):
                raise ctypes.WinError(ctypes.get_last_error())
            copy_inode_metadata(original, destination)
            files += 1
    print(f"Preserved case and inode metadata in {count} directories, {files} files")


if __name__ == "__main__":
    main()
