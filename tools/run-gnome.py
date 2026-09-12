"""Launch GNOME Shell with private system/session buses in a prepared guest root."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid
from session_process import SessionProcess

SESSION = r'''
import json, os, subprocess, sys, time
from gi.repository import Gio, GLib
settings = Gio.Settings.new('org.gnome.shell')
extensions = settings.get_strv('enabled-extensions')
if 'desktop-windows@kinakaze' in extensions:
    settings.set_strv('enabled-extensions', [e for e in extensions if e != 'desktop-windows@kinakaze'])
    Gio.Settings.sync()
# Use one explicit endpoint for IBus and Shell; native X properties do not
# provide the cross-process address discovery used on a conventional X server.
os.environ['IBUS_ADDRESS'] = 'unix:path=/tmp/kinakaze-ibus-' + os.urandom(8).hex()
if os.environ.get('KINAKAZE_SESSION_STATE'):
    import json
    from pathlib import Path
    keys = ('DBUS_SESSION_BUS_ADDRESS', 'DBUS_SYSTEM_BUS_ADDRESS', 'IBUS_ADDRESS',
            'XDG_SESSION_TYPE', 'XDG_CURRENT_DESKTOP', 'XDG_SESSION_DESKTOP', 'TZ')
    Path(os.environ['KINAKAZE_SESSION_STATE']).write_text(
        json.dumps({key: os.environ[key] for key in keys if key in os.environ}))
ibus = subprocess.Popen(['/usr/bin/ibus-daemon', '--replace', '--panel', 'disable',
                         '--xim', '--address=' + os.environ['IBUS_ADDRESS']])
try:
    deadline = time.monotonic() + 30
    connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    while time.monotonic() < deadline:
        if ibus.poll() is not None:
            raise RuntimeError('IBus exited during startup: ' + str(ibus.returncode))
        try:
            reply = connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
                'org.freedesktop.DBus', 'NameHasOwner', GLib.Variant('(s)', ('org.freedesktop.IBus',)),
                GLib.VariantType.new('(b)'), Gio.DBusCallFlags.NONE, 3000, None)
            if reply.unpack()[0]:
                break
        except GLib.Error:
            pass
        time.sleep(0.1)
    else:
        raise RuntimeError('IBus did not acquire its session bus name')
    print('IBus ready: ' + os.environ['IBUS_ADDRESS'], flush=True)
    command = (['/usr/bin/gnome-session', '--builtin', '--session=gnome', '--disable-acceleration-check']
               if os.environ.get('KINAKAZE_SESSION_MANAGER') == '1'
               else ['/usr/bin/gnome-shell', '--x11'])
    shell = subprocess.Popen(command)
    # Apps must inherit this runtime and its IPC namespace, not merely the
    # textual D-Bus address in a separate `worker run` instance.
    applications = []
    for application in json.loads(os.environ.get('KINAKAZE_DESKTOP_APPS', '[]')):
        try:
            applications.append(subprocess.Popen(application))
        except OSError as error:
            print('Could not start ' + repr(application) + ': ' + str(error), file=sys.stderr, flush=True)
    sys.exit(shell.wait())
finally:
    ibus.terminate()
    try:
        ibus.wait(timeout=5)
    except subprocess.TimeoutExpired:
        ibus.kill()
        ibus.wait()
'''

GUEST = r'''
import json, os, subprocess, sys
from pathlib import Path
config = Path(sys.argv[1])
runtime = config.parent / (config.stem + '-runtime')
runtime.mkdir(mode=0o700, exist_ok=True)
os.environ['XDG_RUNTIME_DIR'] = str(runtime)
address = 'unix:path=' + str(config.with_suffix('.socket'))
config.write_text(''' + "'''" + '''<busconfig><type>system</type><listen>{}</listen>
<standard_system_servicedirs/><auth>EXTERNAL</auth>
<policy context="default"><allow user="*"/><allow own="*"/>
<allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>''' + "'''" + r'''.format(address))
bus = subprocess.Popen(['/usr/bin/dbus-daemon', '--nofork', '--print-address=1',
                        '--config-file=' + str(config)], stdout=subprocess.PIPE)
try:
    line = bus.stdout.readline().decode().strip()
    if not line:
        raise RuntimeError('Private system bus did not start: ' + str(bus.wait()))
    os.environ['DBUS_SYSTEM_BUS_ADDRESS'] = line
    os.environ['KINAKAZE_SESSION_STATE'] = str(config.with_suffix('.environment.json'))
    os.environ['KINAKAZE_DESKTOP_APPS'] = json.dumps(APPS)
    os.environ['KINAKAZE_SESSION_MANAGER'] = '1' if SESSION_MANAGER else '0'
    # The worker's generic environment defaults to /bin/sh. Desktop terminals
    # must use the prepared account's login shell, including installed Bash.
    import pwd
    account = pwd.getpwuid(os.getuid())
    login_shell = account.pw_shell
    os.environ.setdefault('XDG_CONFIG_HOME', account.pw_dir + '/.config')
    os.environ.setdefault('XDG_DATA_HOME', account.pw_dir + '/.local/share')
    if login_shell and os.access(login_shell, os.X_OK):
        os.environ['SHELL'] = login_shell
    timezone = Path('/etc/timezone')
    if timezone.is_file():
        os.environ.setdefault('TZ', timezone.read_text().strip())
    os.environ.setdefault('XDG_SESSION_TYPE', 'x11')
    os.environ.setdefault('XDG_CURRENT_DESKTOP', 'GNOME')
    os.environ.setdefault('XDG_SESSION_DESKTOP', 'gnome')
    os.environ.setdefault('GNOME_SHELL_SESSION_MODE', 'user')
    sys.exit(subprocess.call(['/usr/bin/dbus-run-session', '--', '/usr/bin/python3.11', '-c', SESSION]))
finally:
    bus.terminate()
    try:
        bus.wait(timeout=5)
    except subprocess.TimeoutExpired:
        bus.kill()
        bus.wait()
    config.unlink(missing_ok=True)
    config.with_suffix('.environment.json').unlink(missing_ok=True)
'''

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', required=True, type=Path)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--timeout', type=float, default=0,
                        help='Seconds to observe, then stop this session; zero keeps it open')
    parser.add_argument('--report', type=Path)
    parser.add_argument('--app', action='append', nargs='+', default=[],
                        help='Start this program and its arguments inside the desktop session; repeatable')
    parser.add_argument('--session-manager', action='store_true',
                        help='Launch the installed GNOME session manager and its desktop services')
    args = parser.parse_args()
    root, dist = args.root.resolve(), args.dist.resolve()
    for path in (dist / 'worker.exe', root / 'usr/bin/python3.11',
                 root / 'usr/bin/dbus-run-session', root / 'usr/bin/gnome-shell'):
        if not path.is_file():
            raise FileNotFoundError(path)
    if args.timeout < 0:
        parser.error('--timeout must be nonnegative')
    token = 'gnome-session-' + uuid.uuid4().hex
    directory = root / 'tmp'
    directory.mkdir(exist_ok=True)
    script = directory / (token + '.py')
    script.write_text('SESSION = ' + repr(SESSION) + '\nAPPS = ' + repr(args.app)
                      + '\nSESSION_MANAGER = ' + repr(args.session_manager) + '\n' + GUEST,
                      encoding='utf-8', newline='\n')
    logdir = (args.report.parent if args.report else dist / 'logs').resolve()
    logdir.mkdir(parents=True, exist_ok=True)
    stdout, stderr = logdir / (token + '.stdout.log'), logdir / (token + '.stderr.log')
    command = [str(dist / 'worker.exe'), 'run', '--root', str(root), '--dist', str(dist),
               '--', '/usr/bin/python3.11', '/tmp/' + script.name, '/tmp/' + token + '.conf']
    print('Starting GNOME Shell; logs:', stderr, flush=True)
    start, timed_out, interrupted = time.monotonic(), False, False
    with stdout.open('wb') as out, stderr.open('wb') as err:
        session = SessionProcess(command, stdout=out, stderr=err, suspended=True)
        process = session.process
        try:
            session.resume()
            process.wait(timeout=args.timeout or None)
        except subprocess.TimeoutExpired:
            timed_out = True
        except KeyboardInterrupt:
            interrupted = True
        finally:
            # Closing the Shell also ends bus-activated applications and
            # services whose original parent has already exited.
            session.close()
            if process.poll() is None:
                if os.name == 'nt':
                    subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                   creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0), timeout=10)
                else:
                    process.terminate()
                process.wait(timeout=10)
            script.unlink(missing_ok=True)
            (directory / (token + '.conf')).unlink(missing_ok=True)
    text = stderr.read_text(encoding='utf-8', errors='replace')
    started = 'GNOME Shell started at ' in text
    fault = 'guest fault:' in text or 'ERROR: ' in text
    report = dict(started=started, survived_observation=timed_out and started and not fault,
                  process_alive_at_timeout=timed_out,
                  exit_code=process.returncode, timed_out=timed_out, interrupted=interrupted,
                  seconds=round(time.monotonic() - start, 2), stdout=str(stdout), stderr=str(stderr),
                  scope='Shell startup and process survival; does not verify every desktop service or input action')
    if args.report:
        args.report.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report, indent=2))
    return 0 if (report['survived_observation'] or process.returncode == 0 or interrupted) else 1

if __name__ == '__main__':
    sys.exit(main())
