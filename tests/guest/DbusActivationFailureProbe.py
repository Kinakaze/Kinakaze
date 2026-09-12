"""Failed service exec must deliver a D-Bus error without the activation timeout."""
import argparse
import json
from pathlib import Path
import select
import subprocess
import tempfile
import time
from gi.repository import Gio, GLib

parser = argparse.ArgumentParser()
parser.add_argument('--program', help='Optional diagnostic executable known to fail exec')
parser.add_argument('--attempts', type=int, default=12)
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix='dbus-failure-') as directory:
    root = Path(directory)
    services = root / 'services'
    services.mkdir()
    invalid = root / 'invalid-elf'
    invalid.write_bytes(b'\x7fELFtruncated')
    invalid.chmod(0o755)
    program = args.program or str(invalid)
    for index in range(args.attempts):
        name = 'org.kinakaze.FailedActivation' + str(index)
        (services / (name + '.service')).write_text(
            '[D-BUS Service]\nName=' + name + '\nExec=' + program + '\n')
    config = root / 'bus.conf'
    config.write_text('''<busconfig>
      <type>session</type><listen>unix:tmpdir=/tmp</listen>
      <servicedir>''' + str(services) + '''</servicedir>
      <policy context="default"><allow send_destination="*"/>
      <allow receive_sender="*"/><allow own="*"/></policy>
      <limit name="service_start_timeout">5000</limit>
    </busconfig>''')
    with (root / 'bus.log').open('w+') as log:
        bus = subprocess.Popen(['/usr/bin/dbus-daemon', '--nofork', '--print-address=1',
                                '--config-file=' + str(config)], stdout=subprocess.PIPE, stderr=log)
        connection = None
        results = []
        try:
            assert select.select([bus.stdout], [], [], 5)[0], 'private test bus did not publish its address'
            address = bus.stdout.readline().decode().strip()
            assert address, 'private test bus failed to start'
            connection = Gio.DBusConnection.new_for_address_sync(
                address, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT |
                Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)
            for index in range(args.attempts):
                started = time.monotonic()
                try:
                    connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
                        'org.freedesktop.DBus', 'StartServiceByName',
                        GLib.Variant('(su)', ('org.kinakaze.FailedActivation' + str(index), 0)),
                        GLib.VariantType.new('(u)'), Gio.DBusCallFlags.NONE, 3000, None)
                except GLib.Error as error:
                    remote = Gio.DBusError.get_remote_error(error)
                    result = dict(attempt=index, elapsed_ms=round((time.monotonic()-started)*1000, 2),
                                  remote_error=remote, message=str(error))
                    results.append(result)
                    print(json.dumps(result), flush=True)
                    if remote != 'org.freedesktop.DBus.Error.Spawn.ExecFailed':
                        break
                else:
                    raise AssertionError('invalid executable activated successfully')
        finally:
            if connection is not None:
                connection.close_sync(None)
            bus.terminate()
            try:
                bus.wait(timeout=5)
            except subprocess.TimeoutExpired:
                bus.kill()
                bus.wait()
            log.flush()
            log.seek(0)
            print(log.read(), flush=True)
        assert len(results) == args.attempts and all(
            result['remote_error'] == 'org.freedesktop.DBus.Error.Spawn.ExecFailed'
            for result in results), results
print('DBUS_FAILED_ACTIVATION_RETURNS_PROMPTLY', flush=True)
