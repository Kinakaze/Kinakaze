"""Launch installed Minecraft Java client metadata with the Linux V2 runtime.

Uses a separate game directory and official demo mode by default. No account
credentials or existing worlds are read. Libraries must match Mojang metadata.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
from guest_installation import java_home, minecraft_version


def allowed(rules, features):
    result = not rules
    for rule in rules or []:
        platform = rule.get('os', {})
        if platform.get('name', 'linux') != 'linux':
            continue
        if 'arch' in platform and platform['arch'] not in ('x86_64', 'amd64'):
            continue
        if 'version' in platform and not re.search(platform['version'], '6.8.0'):
            continue
        if any(features.get(key, False) != value for key, value in rule.get('features', {}).items()):
            continue
        result = rule['action'] == 'allow'
    return result


def expand(values, variables, features):
    for value in values:
        if isinstance(value, dict):
            if not allowed(value.get('rules'), features):
                continue
            value = value['value']
        for item in value if isinstance(value, list) else [value]:
            def substitute(match):
                if match[1] not in variables:
                    raise ValueError(f'unresolved launch variable {match[1]}')
                return variables[match[1]]
            yield re.sub(r'\$\{([^}]+)\}', substitute, item)


def command(args):
    root = args.root.resolve()
    game = root / 'minecraft'
    version_dir = game / 'versions' / args.version
    metadata = json.loads((version_dir / f'{args.version}.json').read_text(encoding='utf-8'))
    features = {'is_demo_user': True, 'has_custom_resolution': True}
    classpath = []
    for library in metadata['libraries']:
        if not allowed(library.get('rules'), features):
            continue
        artifact = library['downloads'].get('artifact')
        if artifact:
            path = game / 'libraries' / artifact['path']
            if hashlib.sha1(path.read_bytes()).hexdigest() != artifact['sha1']:
                raise ValueError(f'library checksum mismatch: {path}')
            classpath.append('/minecraft/libraries/' + artifact['path'])
    client = version_dir / f'{args.version}.jar'
    if hashlib.sha1(client.read_bytes()).hexdigest() != metadata['downloads']['client']['sha1']:
        raise ValueError(f'client checksum mismatch: {client}')
    classpath.append(f'/minecraft/versions/{args.version}/{args.version}.jar')
    game_directory = '/minecraft/v2-demo'
    for path in (game_directory, '/minecraft/v2-natives/java', '/minecraft/v2-natives/jna',
                 '/minecraft/v2-natives/lwjgl', '/minecraft/v2-natives/netty'):
        (root / path.lstrip('/')).mkdir(parents=True, exist_ok=True)
    variables = {
        'auth_player_name': 'KinakazeDemo', 'auth_uuid': '00000000000000000000000000000000',
        'auth_access_token': '0', 'clientid': '', 'auth_xuid': '',
        'version_name': args.version, 'version_type': metadata.get('type', 'release'),
        'game_directory': game_directory, 'assets_root': '/minecraft/assets',
        'assets_index_name': metadata['assetIndex']['id'],
        'natives_directory': '/minecraft/v2-natives', 'launcher_name': 'KinakazeV2',
        'launcher_version': '2', 'classpath': ':'.join(classpath),
        'resolution_width': str(args.width), 'resolution_height': str(args.height),
    }
    return [str(args.worker.resolve()), 'run', '--root', str(root), '--dist', str(args.dist.resolve()),
            '--cwd', game_directory, '--', args.java, f'-Xmx{args.memory}',
            *expand(metadata['arguments']['jvm'], variables, features),
            metadata['mainClass'], *expand(metadata['arguments']['game'], variables, features)]


def main():
    workspace = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=workspace / 'artifacts/guest-root')
    parser.add_argument('--dist', type=Path, default=workspace / 'dist')
    parser.add_argument('--worker', type=Path, default=workspace / 'target/debug/worker.exe')
    parser.add_argument('--version', help='installed version; auto-detected when unique')
    parser.add_argument('--java', help='absolute guest Java path; auto-detected when unique')
    parser.add_argument('--memory', default='2G')
    parser.add_argument('--width', type=int, default=960)
    parser.add_argument('--height', type=int, default=600)
    parser.add_argument('--print-command', action='store_true')
    args = parser.parse_args()
    args.version = args.version or minecraft_version(args.root)
    args.java = args.java or '/' + java_home(args.root) + '/bin/java'
    invocation = command(args)
    if args.print_command:
        print(json.dumps(invocation, ensure_ascii=False, indent=2))
        return 0
    return subprocess.call(invocation)


if __name__ == '__main__':
    sys.exit(main())
