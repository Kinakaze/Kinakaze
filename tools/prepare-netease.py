"""Prepare the Deepin NetEase client in a separate Kinakaze guest root."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import sys
import urllib.request

WORKSPACE = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--dist', type=Path, default=WORKSPACE / 'artifacts/netease-dist')
parser.add_argument('--base', type=Path, default=WORKSPACE / 'artifacts/netease-cloud-music')
parser.add_argument('--support-root', type=Path, default=WORKSPACE / 'artifacts/gnome-startup-root',
                    help='Prepared guest root supplying base configuration and dependencies absent from the lock')
args = parser.parse_args()
BASE = args.base.resolve()
BASE.mkdir(parents=True, exist_ok=True)
SOURCE = BASE / 'source-v2'
ROOT = BASE / 'rootfs'
DIST = args.dist.resolve()
DESKTOP = args.support_root.resolve()
if not (DIST / 'worker.exe').is_file() or not (DESKTOP / 'usr/bin/busybox').is_file():
    parser.error('Build/package the runtime and prepare the support root first; see docs/netease-cloud-music-2026-09-22.md')
sys.path[:0] = [str(WORKSPACE / 'tools'), str(WORKSPACE / 'tools/guest-deps')]
from guest_deps import PackageCache, ar_members, tar_members, resolve_member, relative_path
from native_image import modules

spec = importlib.util.spec_from_file_location('prepare_root', WORKSPACE / 'tools/prepare-root.py')
prepare_root = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare_root)
records = json.loads((WORKSPACE / 'tools/guest-deps/netease.lock.json').read_text(encoding='utf-8'))
provenance = []

def deepin_package(name, selected_plugins=None):
    record = records[name]
    url = 'https://community-packages.deepin.com/deepin/' + record['Filename']
    archive = BASE / Path(record['Filename']).name
    if not archive.is_file():
        print('Downloading', name, record['Version'], flush=True)
        with urllib.request.urlopen(url, timeout=60) as response:
            archive.write_bytes(response.read())
    blob = archive.read_bytes()
    if len(blob) != int(record['Size']) or hashlib.sha256(blob).hexdigest() != record['SHA256']:
        raise ValueError('Package size or SHA-256 mismatch: ' + name)
    members = ar_members(blob)
    payload = tar_members(next(data for key, data in members.items() if key.startswith('data.tar')))
    count = 0
    for member in payload:
        if selected_plugins is not None and '/vlc/plugins/' in member and Path(member).name not in selected_plugins:
            continue
        try:
            _, data = resolve_member(payload, member)
        except KeyError:
            print('External/directory alias omitted:', member, flush=True)
            continue
        target = SOURCE / relative_path(member)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        count += 1
    provenance.append(dict(package=name, version=record['Version'], url=url,
                           sha256=record['SHA256'], files=count,
                           selected_plugins=sorted(selected_plugins) if selected_plugins is not None else None))
    print('Verified and extracted', name, count, flush=True)

for name in ['netease-cloud-music', 'libqcef1', 'libqt5webchannel5', 'libvlc5',
             'libvlccore9', 'libtag1v5-vanilla', 'libgconf-2-4', 'libqt5x11extras5', 'libqt5qml5',
             'libdbus-glib-1-2', 'libicu63', 'libidn11', 'fonts-wqy-zenhei', 'libflac8', 'libfaad2', 'libssl1.1', 'vlc-data',
             'libqt5core5a', 'libqt5gui5', 'libqt5widgets5', 'libqt5network5', 'libqt5dbus5',
             'libqt5xml5', 'libqt5printsupport5', 'libqt5opengl5', 'libqt5svg5', 'libqt5sql5', 'libdouble-conversion1']:
    deepin_package(name)
deepin_package('vlc-plugin-base', {
    'lib' + name + '_plugin.so' for name in (
        'filesystem', 'http', 'https', 'tcp', 'access_imem', 'imem',
        'alsa', 'pulse', 'afile', 'amem', 'audio_format', 'float_mixer', 'integer_mixer',
        'equalizer', 'gain', 'normvol', 'scaletempo', 'simple_channel_mixer',
        'trivial_channel_mixer', 'ugly_resampler', 'samplerate',
        'araw', 'adpcm', 'mpg123', 'faad', 'flac', 'vorbis', 'opus', 'g711',
        'wav', 'aiff', 'au', 'es', 'flacsys', 'ogg', 'mp4', 'rawaud', 'playlist',
        'mpeg4audio', 'mpegaudio', 'packetizer_copy', 'cache_read', 'cache_block',
        'record', 'prefetch', 'inflate', 'gnutls', 'logger', 'xml', 'taglib',
        'memory', 'file',
    )
})
(BASE / 'packages.json').write_text(json.dumps(provenance, indent=2) + '\n', encoding='utf-8')

class Plan(prepare_root.RootPlan):
    def _resolve_local(self, dependency, origin, runpaths):
        result = super()._resolve_local(dependency, origin, runpaths)
        if result:
            return result
        if self.cache.resolve(dependency) is not None:
            return None
        for directory in ('usr/lib', 'usr/lib/x86_64-linux-gnu', 'lib/x86_64-linux-gnu', 'lib'):
            relative = Path(directory) / dependency
            existing = DESKTOP / relative
            if existing.is_file():
                target = SOURCE / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(existing, target)
                return target.resolve()
        return None

for relative in ['usr/bin/busybox', 'etc/passwd', 'etc/group', 'etc/hosts', 'etc/resolv.conf',
                 'etc/nsswitch.conf', 'etc/ssl/certs/ca-certificates.crt']:
    target = SOURCE / relative
    if (DESKTOP / relative).is_file():
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(DESKTOP / relative, target)
cache = PackageCache(WORKSPACE / 'tools/guest-deps/dependencies.lock.json', WORKSPACE / 'artifacts/guest-deps', False)
plan = Plan(SOURCE, ROOT, cache, set(modules(DIST)), 'usr/lib/jvm/unused')
plan.search.append(SOURCE / 'usr/lib/x86_64-linux-gnu/qcef')
plan.copy_tree('usr')
plan.copy_tree('etc')
# The current worker did not find CEF via the private LD_LIBRARY_PATH.
# Preserve its original resource directory and expose the same bytes in a
# standard library directory, without substituting another CEF build.
plan.copy_source('usr/lib/x86_64-linux-gnu/qcef/libcef.so', 'usr/lib/x86_64-linux-gnu/libcef.so')
plan.copy_source('usr/bin/busybox', 'bin/busybox')
plan.copy_source('usr/bin/busybox', 'bin/sh')
for name in ['fontconfig-config', 'fonts-dejavu-core', 'dbus']:
    plan.copy_package(name)
plan.close_dependencies()
count = plan.install(['netease-cloud-music'], BASE / 'dependencies.json')
print('Prepared', count, 'files;', len(plan.edges), 'dependency edges', flush=True)
