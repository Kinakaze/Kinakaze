"""Discover installed guest versions without embedding a developer's toolchain."""
from pathlib import Path


def java_home(root):
    root = Path(root)
    candidates = sorted(path.parent.parent for path in (root / 'usr/lib/jvm').glob('*/bin/java') if path.is_file())
    if len(candidates) != 1:
        raise ValueError(f'expected one installed JRE under {root / "usr/lib/jvm"}; select it explicitly')
    return candidates[0].relative_to(root).as_posix()


def minecraft_version(root):
    versions = Path(root) / 'minecraft/versions'
    candidates = sorted(path.name for path in versions.iterdir()
                        if path.is_dir() and (path / f'{path.name}.json').is_file()) if versions.is_dir() else []
    if len(candidates) != 1:
        raise ValueError(f'expected one installed Minecraft version under {versions}; select it explicitly')
    return candidates[0]
