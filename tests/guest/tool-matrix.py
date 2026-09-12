"""Run independent, bounded Linux tool probes; missing inputs never count as passes."""
from __future__ import annotations
from distribution import runtime_image

import argparse
from collections import Counter
from contextlib import nullcontext
from datetime import datetime, timezone
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import threading
import time
from host_fixtures import pointer_control

WORKSPACE = Path(__file__).resolve().parents[2]


class Fixture(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'KINAKAZE_HTTP_OK\n' if self.path == '/fixture' else b'not found\n'
        self.send_response(200 if self.path == '/fixture' else 404)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def atomic_report(path, report):
    temporary = path.with_suffix('.json.tmp')
    temporary.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    temporary.replace(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', type=Path, default=WORKSPACE / 'target/debug/worker.exe')
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', type=Path, default=WORKSPACE / 'dist')
    parser.add_argument('--manifest', type=Path, default=Path(__file__).with_suffix('.json'))
    parser.add_argument('--report', type=Path, default=WORKSPACE / 'artifacts/tool-matrix.json')
    parser.add_argument('--only', action='append', default=[], help='probe ID (repeatable)')
    parser.add_argument('--timeout', type=float, default=30)
    args = parser.parse_args()
    if not 0 < args.timeout <= 600:
        parser.error('--timeout must be within (0, 600] seconds')
    worker, root, dist, report_path = (path.resolve() for path in (args.worker, args.root, args.dist, args.report))
    if not worker.is_file() or not root.is_dir() or not (runtime_image(dist)).is_file():
        parser.error('worker, prepared root and runtime distribution must exist')
    manifest = json.loads(args.manifest.read_text(encoding='utf-8'))
    probes = manifest['probes']
    ids = [probe['id'] for probe in probes]
    if manifest['schema_version'] != 1 or len(set(ids)) != len(ids):
        parser.error('invalid manifest version or duplicate IDs')
    if unknown := set(args.only) - set(ids):
        parser.error(f'unknown probes: {sorted(unknown)}')
    for probe in probes:
        if not probe['id'].isascii() or not probe['id'].replace('-', '').isalnum():
            parser.error('probe IDs must contain only ASCII letters, digits and hyphens')
        if probe['level'] not in ('startup', 'behavior') or not probe.get('expect'):
            parser.error('each probe needs a level and expected output')
        if probe.get('host_fixture') not in (None, 'pointer-control'):
            parser.error('unknown host fixture')
        for name in probe['requires']:
            if not name.startswith('/') or not (root / name.lstrip('/')).resolve().is_relative_to(root):
                parser.error('required guest paths must stay within the root')
        if source_name := probe.get('source'):
            source = (Path(__file__).parent / source_name).resolve()
            if not source.is_relative_to(Path(__file__).parent.resolve()) or not source.is_file() or source.stat().st_size > 16384:
                parser.error('probe sources must be local test files of at most 16 KiB')
    report_path.parent.mkdir(parents=True, exist_ok=True)
    logs = report_path.parent / (report_path.stem + '-logs')
    logs.mkdir(exist_ok=True)
    report = {
        'schema_version': 1, 'created_utc': datetime.now(timezone.utc).isoformat(),
        'root': str(root), 'dist': str(dist), 'worker_sha256': digest(worker),
        'runtime_sha256': digest(runtime_image(dist)),
        'manifest_sha256': digest(args.manifest), 'results': [],
    }
    selected = [probe for probe in probes if not args.only or probe['id'] in args.only]
    needs_http = any(any('{http_url}' in arg for arg in probe['argv'])
                     and all((root / path.lstrip('/')).is_file() for path in probe['requires'])
                     for probe in selected)
    server, thread, http_url = None, None, ''
    if needs_http:
        server = ThreadingHTTPServer(('127.0.0.1', 0), Fixture)
        http_url = f'http://127.0.0.1:{server.server_port}'
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
    try:
        for probe in selected:
            result = {'id': probe['id'], 'level': probe['level']}
            missing = [path for path in probe['requires'] if not (root / path.lstrip('/')).is_file()]
            if missing:
                result.update(status='missing', missing=missing)
            else:
                argv = [arg.replace('{http_url}', http_url) for arg in probe['argv']]
                if source_name := probe.get('source'):
                    source = Path(__file__).parent / source_name
                    content = source.read_text(encoding='utf-8')
                    argv = [arg.replace('{source}', content) for arg in argv]
                    result['source_sha256'] = digest(source)
                result['argv'] = argv
                command = [str(worker), 'run', '--root', str(root), '--dist', str(dist), '--', *argv]
                output_path = logs / (probe['id'] + '.stdout.log')
                error_path = logs / (probe['id'] + '.stderr.log')
                started = time.monotonic()
                # Files avoid pipe deadlocks and retaining arbitrary guest output in RAM.
                guard = pointer_control() if probe.get('host_fixture') == 'pointer-control' else nullcontext()
                with guard, output_path.open('wb') as output, error_path.open('wb') as error:
                    process = subprocess.Popen(command, stdout=output, stderr=error,
                                               creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
                    try:
                        result['exit_code'] = process.wait(timeout=args.timeout)
                        result['status'] = 'passed' if result['exit_code'] == 0 else 'failed'
                    except subprocess.TimeoutExpired:
                        # Init watches this exact supervisor and closes its worker Job on death.
                        process.kill()
                        process.wait(timeout=5)
                        result['status'] = 'timeout'
                result['elapsed_ms'] = round((time.monotonic() - started) * 1000, 1)
                result['stdout'], result['stderr'] = str(output_path), str(error_path)
                # Stream the marker check too; diagnostics can exceed a small tail buffer.
                marker = probe['expect'].encode('utf-8')
                def contains_marker(path):
                    with path.open('rb') as stream:
                        tail = b''
                        while chunk := stream.read(65536):
                            chunk = tail + chunk
                            if marker in chunk:
                                return True
                            tail = chunk[-len(marker):]
                    return False
                result['expected_output_found'] = contains_marker(output_path) or contains_marker(error_path)
                if result['status'] == 'passed' and not result['expected_output_found']:
                    result['status'] = 'failed'
            report['results'].append(result)
            report['summary'] = dict(Counter(row['status'] for row in report['results']))
            atomic_report(report_path, report)
            print(f"{result['id']}: {result['status']} ({result['level']})", flush=True)
    finally:
        if server:
            server.shutdown()
            server.server_close()
            thread.join()
    print(json.dumps(report['summary']), flush=True)
    return int(any(row['status'] != 'passed' for row in report['results']))


if __name__ == '__main__':
    raise SystemExit(main())
