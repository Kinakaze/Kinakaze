"""A proc fd directory alias must retain cwd independently of the source fd."""
import os
from pathlib import Path
import subprocess
import tempfile


initial = os.getcwd()
with tempfile.TemporaryDirectory(prefix='proc-fd-cwd-') as directory:
    root = Path(directory)
    (root / '子目录 space').mkdir()
    (root / '子目录 space/value.txt').write_text('cwd retained', encoding='utf-8')
    for suffix in ('', '/子目录 space'):
        fd = os.open(root, os.O_PATH | os.O_DIRECTORY | os.O_CLOEXEC)
        try:
            os.chdir(f'/proc/self/fd/{fd}' + suffix)
        finally:
            os.close(fd)
        expected = str(root) + suffix
        assert os.getcwd() == expected, (os.getcwd(), expected)
        child = 'import os,sys; assert os.getcwd()==sys.argv[1], (os.getcwd(),sys.argv[1]); print("CWD_CHILD_OK")'
        assert subprocess.check_output(['/usr/bin/python3', '-c', child, expected]) == b'CWD_CHILD_OK\n'
        os.chdir(initial)
    # Renaming the directory must update getcwd and the next exec as well.
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        os.chdir(f'/proc/{os.getpid()}/fd/{fd}/子目录 space')
    finally:
        os.close(fd)
    renamed = root / 'renamed'
    os.rename(root / '子目录 space', renamed)
    assert os.getcwd() == str(renamed)
    child = 'from pathlib import Path; assert Path("value.txt").read_text()=="cwd retained"; print("CWD_RENAMED_OK")'
    assert subprocess.check_output(['/usr/bin/python3', '-c', child]) == b'CWD_RENAMED_OK\n'
    os.chdir(initial)
print('PROC_FD_CWD_CLOSE_RENAME_EXEC_OK')
