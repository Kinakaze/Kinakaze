"""Cwd-relative opens preserve inode identity, dot components and dirfd lookup."""
import errno
import os
import shutil
import tempfile

directory = tempfile.mkdtemp(prefix="relative-native-read-", dir="/var/tmp")
saved = os.getcwd()
try:
    os.mkdir(directory + "/sub")
    os.mkdir(directory + "/target")
    for name, content in (("payload", b"parent"), ("sub/payload", b"original"),
                          ("target/payload", b"dirfd")):
        with open(directory + "/" + name, "wb") as stream:
            stream.write(content)
    os.symlink("../payload", directory + "/sub/link")
    os.symlink("../target", directory + "/sub/jump")
    os.chdir(directory + "/sub")
    fd = os.open("payload", os.O_RDONLY | os.O_CLOEXEC)
    alias = os.dup(fd)
    inode = os.fstat(fd).st_ino
    os.rename("payload", "old")
    os.unlink("old")
    with open("payload", "wb") as stream:
        stream.write(b"replacement")
    assert os.fstat(fd).st_ino == inode
    assert os.read(fd, 3) == b"ori"
    assert os.read(alias, 5) == b"ginal"
    os.close(alias)
    os.close(fd)
    for name, expected in (("payload", b"replacement"), ("./payload", b"replacement"),
                           ("link", b"parent"), ("jump/../payload", b"parent")):
        with open(name, "rb") as stream:
            assert stream.read() == expected, name
    for name, flags, expected in (("link", os.O_RDONLY | os.O_NOFOLLOW, errno.ELOOP),
                                  ("payload/", os.O_RDONLY, errno.ENOTDIR),
                                  ("missing", os.O_RDONLY, errno.ENOENT)):
        try:
            os.open(name, flags)
        except OSError as error:
            assert error.errno == expected, (name, error)
        else:
            raise AssertionError(name)
    parent = os.open(directory + "/target", os.O_RDONLY | os.O_DIRECTORY)
    os.rename(directory + "/target", directory + "/moved-target")
    os.mkdir(directory + "/target")
    with open(directory + "/target/payload", "wb") as stream:
        stream.write(b"different dirfd")
    fd = os.open("payload", os.O_RDONLY, dir_fd=parent)
    assert os.read(fd, 64) == b"dirfd"
    os.close(fd)
    os.close(parent)
finally:
    os.chdir(saved)
    shutil.rmtree(directory)
print("RELATIVE_NATIVE_READ_OK", flush=True)
