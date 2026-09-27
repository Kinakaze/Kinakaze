"""Prepare and validate a small, version-bound GNOME IBus resource override."""
import hashlib
import json
from pathlib import Path


LIBRARY = Path('usr/lib/gnome-shell/libgnome-shell.so')
DIRECTORY = Path('usr/share/kinakaze/gnome-shell')
RESOURCE = DIRECTORY / 'ibusManager.js'
MANIFEST = DIRECTORY / 'ibusManager.json'
VERSION = 1

QUEUE_SPAWN = """    async _queueSpawn() {
        const isSystemdService = await this._ibusSystemdServiceExists();
        if (!isSystemdService)
            this._spawn(Meta.is_wayland_compositor() ? [] : ['--xim']);
    }"""


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def prepare_ibus_override(root, source):
    """Keep daemon startup/restart, avoiding a duplicate spawn once connected.

    Only the recognized method is changed; an unknown upstream layout is skipped.
    The manifest binds this generated resource to the exact installed Shell ELF.
    """
    text = source.decode('utf-8')
    if text.count(QUEUE_SPAWN) != 1:
        return False
    replacement = QUEUE_SPAWN.replace(
        'if (!isSystemdService)',
        'if (!isSystemdService && !this._ibus.is_connected())')
    patched = text.replace(QUEUE_SPAWN, replacement).encode('utf-8')
    manifest = dict(version=VERSION, library_sha256=sha256((root / LIBRARY).read_bytes()),
                    source_sha256=sha256(source), resource_sha256=sha256(patched))
    (root / DIRECTORY).mkdir(parents=True, exist_ok=True)
    # Publish the manifest last. A launcher racing preparation ignores a pair
    # whose hashes do not match, and runs the original installed resource.
    for relative, data in ((RESOURCE, patched),
                           (MANIFEST, (json.dumps(manifest, indent=2) + '\n').encode('utf-8'))):
        destination = root / relative
        temporary = destination.with_suffix(destination.suffix + '.tmp')
        temporary.write_bytes(data)
        temporary.replace(destination)
    return True


def ibus_resource_overlay(root):
    """Return an overlay only when both its input ELF and output still match."""
    try:
        manifest = json.loads((root / MANIFEST).read_text(encoding='utf-8'))
        if (manifest['version'] != VERSION
                or sha256((root / LIBRARY).read_bytes()) != manifest['library_sha256']
                or sha256((root / RESOURCE).read_bytes()) != manifest['resource_sha256']):
            return None
    except (OSError, ValueError, KeyError, TypeError):
        return None
    return '/org/gnome/shell/misc/ibusManager.js=/' + RESOURCE.as_posix()
