"""Find distribution images through their filenames and native exports."""
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'tools'))
from native_image import modules


def module_image(dist, soname):
    path = Path(dist) / 'rootfs/lib' / soname
    if not path.is_file():
        raise FileNotFoundError(path)
    return path


def runtime_image(dist):
    matches = [name for name, exports in modules(dist).items() if 'kinakaze_runtime_open_v1' in exports]
    if len(matches) != 1:
        raise ValueError(f'Expected one runtime API owner in {dist}: {matches}')
    return module_image(dist, matches[0])
