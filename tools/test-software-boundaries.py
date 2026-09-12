"""Exercise installed desktop applications with disposable, reproducible files.

GUI success means a submitted frame, an optional matching title, and normal
exit after WM_CLOSE. Screenshots still require inspection for document content.
No application is connected to an existing desktop's D-Bus session.
"""
import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import time
import uuid
import zipfile

from session_process import SessionProcess


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def fixtures(directory):
    from PIL import Image, ImageDraw
    text = 'KINAKAZE_BOUNDARY_OK\n中文路径、空格与 emoji 😀\r\nlast line without newline'
    (directory / '边界 sample.txt').write_bytes(text.encode('utf-8'))
    (directory / 'large.txt').write_text(('A' * 255 + '\n') * 4096, encoding='utf-8')
    (directory / 'empty.txt').write_bytes(b'')
    picture = Image.new('RGB', (640, 360), '#182943')
    drawing = ImageDraw.Draw(picture)
    drawing.rectangle((24, 24, 300, 300), fill='#ef803b')
    drawing.ellipse((340, 24, 616, 300), fill='#4ad5b6')
    drawing.text((24, 324), 'KINAKAZE_BOUNDARY_IMAGE', fill='white')
    picture.save(directory / '边界 image.png')
    picture.save(directory / 'image.jpg')
    picture.save(directory / 'image.webp', lossless=True)
    (directory / 'truncated.png').write_bytes((directory / '边界 image.png').read_bytes()[:45])
    content = b'BT /F1 24 Tf 48 720 Td (KINAKAZE PDF BOUNDARY) Tj 0 -40 Td (One page: 42) Tj ET\n'
    objects = [b'<< /Type /Catalog /Pages 2 0 R >>',
               b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
               b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>',
               b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
               b'<< /Length ' + str(len(content)).encode() + b' >>\nstream\n' + content + b'endstream']
    pdf, offsets = bytearray(b'%PDF-1.4\n'), [0]
    for i, obj in enumerate(objects, 1):
        offsets.append(len(pdf))
        pdf.extend(str(i).encode() + b' 0 obj\n' + obj + b'\nendobj\n')
    xref = len(pdf)
    pdf.extend(b'xref\n0 6\n0000000000 65535 f \n')
    for offset in offsets[1:]:
        pdf.extend(f'{offset:010d} 00000 n \n'.encode())
    pdf.extend(f'trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n'.encode())
    (directory / 'sample.pdf').write_bytes(pdf)
    (directory / 'broken.pdf').write_bytes(b'%PDF-1.4\ntruncated fixture\n')
    members = {'边界 sample.txt': text.encode('utf-8'), 'nested/empty.txt': b''}
    with tarfile.open(directory / 'sample.tar.gz', 'w:gz') as archive:
        for name, data in members.items():
            entry = tarfile.TarInfo(name)
            entry.size = len(data)
            archive.addfile(entry, io.BytesIO(data))
    with zipfile.ZipFile(directory / 'sample.zip', 'w', zipfile.ZIP_DEFLATED) as archive:
        for name, data in members.items():
            archive.writestr(name, data)
    (directory / 'index.html').write_text('<!doctype html><title>Boundary</title><p id="result">KINAKAZE_BROWSER_OK</p>', encoding='utf-8')
    return members


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--dist', required=True, type=Path)
    parser.add_argument('--output-dir', required=True, type=Path)
    parser.add_argument('--only', action='append')
    parser.add_argument('--timeout', type=float, default=30)
    parser.add_argument('--close-timeout', type=float, default=20)
    args = parser.parse_args()
    if not 0 < args.timeout <= 300:
        parser.error('timeout must be within (0, 300]')
    if not 0 < args.close_timeout <= 60:
        parser.error('close timeout must be within (0, 60]')
    root, dist, output = (p.resolve() for p in (args.root, args.dist, args.output_dir))
    if not root.is_dir() or not (dist / 'worker.exe').is_file():
        parser.error('prepared root and worker must exist')
    guest = '/tmp/software-boundaries-' + uuid.uuid4().hex
    stage = root / guest.lstrip('/')
    stage.mkdir(parents=True)
    output.mkdir(parents=True, exist_ok=True)
    members = fixtures(stage)
    cases = []

    def gui(name, app, *argv, title=None, scope='file-open'):
        cases.append(dict(id=name, mode='gui', argv=[app, *argv], title=title, scope=scope))

    def cli(name, app, *argv, marker=None, expected_exit=0, verify=None):
        cases.append(dict(id=name, mode='cli', argv=[app, *argv], marker=marker,
                          expected_exit=expected_exit, verify=verify, scope='behavior'))

    for app in ('gedit', 'mousepad'):
        gui(app + '-utf8', '/usr/bin/' + app, guest + '/边界 sample.txt', title='sample.txt')
        gui(app + '-large', '/usr/bin/' + app, guest + '/large.txt', title='large.txt')
    for name in ('边界 image.png', 'image.jpg', 'image.webp', 'truncated.png'):
        gui('eog-' + ('png' if name.startswith('边界') else name.replace('.', '-')),
            '/usr/bin/eog', guest + '/' + name,
            title=None if name.startswith('truncated') else name,
            scope='malformed-file-observation' if name.startswith('truncated') else 'file-open')
    gui('evince-pdf', '/usr/bin/evince', guest + '/sample.pdf', title='sample.pdf')
    gui('evince-broken-pdf', '/usr/bin/evince', guest + '/broken.pdf', scope='malformed-file-observation')
    for suffix in ('tar.gz', 'zip'):
        gui('file-roller-' + suffix.replace('.', '-'), '/usr/bin/file-roller',
            guest + '/sample.' + suffix, title='sample.' + suffix)
        cli('file-roller-extract-' + suffix.replace('.', '-'), '/usr/bin/file-roller',
            '--force', '--extract-to=' + guest + '/extract-' + suffix, guest + '/sample.' + suffix,
            verify='extract-' + suffix)
    gui('nautilus-directory', '/usr/bin/nautilus', '--new-window', guest, scope='directory-open')
    gui('calculator-window', '/usr/bin/gnome-calculator', '--equation=6*7', scope='startup-and-close')
    cli('calculator-arithmetic', '/usr/bin/gnome-calculator', '--solve=6*7', marker='42')
    cli('calculator-precedence', '/usr/bin/gnome-calculator', '--solve=(2+3)^4', marker='625')
    cli('gjs-runtime', '/usr/bin/gjs', '-c',
        'const {Gio, GLib} = imports.gi; let [ok, bytes] = GLib.file_get_contents(ARGV[0]); '
        'if (!ok || imports.byteArray.toString(bytes).indexOf("KINAKAZE_BOUNDARY_OK") < 0) throw Error("file"); '
        'if (JSON.parse(JSON.stringify({value: 42})).value !== 42) throw Error("json"); '
        'print("GJS_FILE_JSON_OK");', guest + '/边界 sample.txt', marker='GJS_FILE_JSON_OK')
    cli('firefox-version', '/opt/firefox/firefox', '--version', marker='Mozilla Firefox')
    cli('chrome-version', '/opt/google/chrome/chrome', '--version', marker='Google Chrome')
    cli('chrome-headless', '/opt/google/chrome/chrome', '--headless', '--no-first-run',
        '--disable-background-networking', '--user-data-dir=' + guest + '/chrome-profile',
        '--dump-dom', 'file://' + guest + '/index.html', marker='KINAKAZE_BROWSER_OK')
    if args.only and set(args.only) - {case['id'] for case in cases}:
        parser.error('unknown case: ' + repr(set(args.only) - {case['id'] for case in cases}))
    report = dict(created_utc=datetime.now(timezone.utc).isoformat(), root=str(root), dist=str(dist),
                  staging=str(stage), scope=__doc__.strip(), results=[],
                  worker_sha256=digest(dist / 'worker.exe'),
                  native_images={p.name: digest(p) for p in (dist / 'rootfs/lib').iterdir() if p.is_file()},
                  fixtures={p.name: digest(p) for p in stage.iterdir() if p.is_file()})
    for case in cases:
        if args.only and case['id'] not in args.only:
            continue
        result = dict(case, status='missing')
        executable = root / case['argv'][0].lstrip('/')
        if executable.is_file():
            result['executable_sha256'] = digest(executable)
            home = stage / (case['id'] + '-home')
            for sub in ('config', 'cache', 'data', 'runtime'):
                (home / sub).mkdir(parents=True, exist_ok=True)
            guest_home = guest + '/' + home.name
            command = ['/usr/bin/env', 'HOME=' + guest_home, 'XDG_CONFIG_HOME=' + guest_home + '/config',
                       'XDG_CACHE_HOME=' + guest_home + '/cache', 'XDG_DATA_HOME=' + guest_home + '/data',
                       'XDG_RUNTIME_DIR=' + guest_home + '/runtime', '/bin/sh', '-c',
                       '/bin/busybox chmod 700 "$XDG_RUNTIME_DIR" && exec /usr/bin/dbus-run-session -- "$@"',
                       'software-boundary', *case['argv']]
            started = time.monotonic()
            logfile = output / (case['id'] + '.log')
            if case['mode'] == 'gui':
                details = output / (case['id'] + '.json')
                command = [sys.executable, str(Path(__file__).with_name('test-application-startup.py')),
                           '--root', str(root), '--dist', str(dist), '--output', str(details),
                           '--timeout', str(args.timeout), '--capture', '--settle-seconds', '2',
                           '--close-timeout', str(args.close_timeout),
                           *(['--title-contains', case['title']] if case['title'] else []), '--', *command]
                with (output / (case['id'] + '.driver.log')).open('wb') as log:
                    child = SessionProcess(command, stdout=log, stderr=log)
                    try:
                        result['exit_code'] = child.process.wait(timeout=args.timeout + args.close_timeout + 17)
                        result['status'] = 'observed' if result['exit_code'] == 0 else 'failed'
                    except subprocess.TimeoutExpired:
                        result['status'] = 'timeout'
                    finally:
                        child.close()
                if details.exists():
                    result['observation'] = json.loads(details.read_text(encoding='utf-8'))
            else:
                with logfile.open('wb') as log:
                    child = SessionProcess([str(dist / 'worker.exe'), 'run', '--root', str(root),
                                            '--dist', str(dist), '--', *command], stdout=log, stderr=log)
                    try:
                        result['exit_code'] = child.process.wait(timeout=args.timeout)
                        result['status'] = 'passed' if result['exit_code'] == case['expected_exit'] else 'failed'
                    except subprocess.TimeoutExpired:
                        result['status'] = 'timeout'
                    finally:
                        child.close()
                if case['marker']:
                    result['marker_found'] = case['marker'].encode() in logfile.read_bytes()
                    if not result['marker_found'] and result['status'] == 'passed':
                        result['status'] = 'failed'
                if case['verify']:
                    extracted = stage / case['verify']
                    result['extracted_files_match'] = all((extracted / name).is_file() and
                        (extracted / name).read_bytes() == data for name, data in members.items())
                    if not result['extracted_files_match'] and result['status'] == 'passed':
                        result['status'] = 'failed'
            result['elapsed_seconds'] = round(time.monotonic() - started, 3)
            result['log'] = str(logfile)
        report['results'].append(result)
        report['summary'] = dict(Counter(r['status'] for r in report['results']))
        pending = output / 'results.json.tmp'
        pending.write_text(json.dumps(report, ensure_ascii=True, indent=2) + '\n', encoding='utf-8')
        pending.replace(output / 'results.json')
        print(case['id'] + ': ' + result['status'], flush=True)
    print(json.dumps(report['summary']), flush=True)
    return int(any(r['status'] not in ('observed', 'passed') for r in report['results']))


if __name__ == '__main__':
    raise SystemExit(main())
