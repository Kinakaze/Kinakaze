"""Real Debian tools and desktop services in disposable guest directories."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def run(*args, data=None, status=0):
    result = subprocess.run(args, input=data, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=35)
    if result.returncode != status:
        raise AssertionError((args, result.returncode, result.stdout[-2048:], result.stderr[-4096:]))
    return result.stdout


def core_files():
    Path('source').write_bytes(b'original data\n')
    run('/bin/mkdir', 'directory')
    run('/bin/cp', '--preserve=mode,timestamps', 'source', 'directory/copy')
    assert Path('directory/copy').read_bytes() == Path('source').read_bytes()
    run('/bin/mv', 'directory/copy', 'directory/moved')
    run('/bin/ln', 'directory/moved', 'hard')
    run('/bin/ln', '-s', 'directory/moved', 'soft')
    assert run('/bin/readlink', 'soft') == b'directory/moved\n'
    assert Path('hard').stat().st_ino == Path('directory/moved').stat().st_ino
    run('/bin/chmod', '640', 'directory/moved')
    assert run('/usr/bin/stat', '-c', '%a:%h', 'hard').strip() == b'640:2'
    assert run('/bin/cat', 'soft') == b'original data\n'
    run('/bin/rm', 'soft', 'hard', 'directory/moved')
    run('/bin/rmdir', 'directory')
    assert not Path('directory').exists()


def core_text():
    assert run('/usr/bin/sort', '-n', data=b'9\n2\n10\n2\n') == b'2\n2\n9\n10\n'
    assert run('/usr/bin/uniq', '-c', data=b'a\na\nb\n').split() == [b'2',b'a',b'1',b'b']
    assert run('/usr/bin/cut', '-d:', '-f2', data=b'key:value:tail\n') == b'value\n'
    assert run('/usr/bin/tr', 'a-z', 'A-Z', data=b'abc\n') == b'ABC\n'
    Path('left').write_bytes(b'a 1\nb 2\n'); Path('right').write_bytes(b'a x\nc y\n')
    assert run('/usr/bin/join', 'left', 'right') == b'a 1 x\n'
    assert run('/usr/bin/paste', '-d:', 'left', 'right') == b'a 1:a x\nb 2:c y\n'
    assert run('/usr/bin/head', '-n1', 'left') == b'a 1\n'
    assert run('/usr/bin/tail', '-n1', 'left') == b'b 2\n'
    assert run('/usr/bin/wc', '-l', 'left').split()[0] == b'2'
    assert run('/usr/bin/seq', '2', '2', '6') == b'2\n4\n6\n'


def core_streams():
    data = bytes(range(256))*257
    Path('input').write_bytes(data)
    run('/bin/dd', 'if=input', 'of=copy', 'bs=4096', 'status=none')
    assert Path('copy').read_bytes() == data
    run('/usr/bin/split', '-b', '4096', 'input', 'part-')
    assert b''.join(p.read_bytes() for p in sorted(Path('.').glob('part-*'))) == data
    assert run('/usr/bin/tee', 'tee-copy', data=data) == data
    assert Path('tee-copy').read_bytes() == data
    for codec in ['base32','base64']:
        encoded = run('/usr/bin/'+codec, data=data)
        assert run('/usr/bin/'+codec, '-d', data=encoded) == data
    run('/usr/bin/truncate', '-s', '1048576', 'sparse')
    assert Path('sparse').stat().st_size == 1048576
    with Path('sparse').open('rb') as f:
        f.seek(1048575); assert f.read() == b'\0'


def hashes():
    data = bytes(range(256))*31 + b'crypto-independent fixture'
    Path('input').write_bytes(data)
    for tool, name in [('md5sum','md5'),('sha1sum','sha1'),('sha256sum','sha256'),('sha512sum','sha512'),('b2sum','blake2b')]:
        expected = hashlib.new(name,data).hexdigest().encode()
        output = run('/usr/bin/'+tool, 'input')
        assert output.split()[0] == expected
        Path('checksums').write_bytes(output)
        assert b'OK' in run('/usr/bin/'+tool, '-c', 'checksums')


def compressor(name):
    exe={'gzip':'/bin/gzip','bzip2':'/bin/bzip2','xz':'/usr/bin/xz'}[name]
    data = bytes(range(256))*819 + b'end'
    args=['-T2'] if name=='xz' else []
    encoded=run(exe,*args,'-c',data=data)
    assert len(encoded)<len(data)//4 and encoded != data
    assert run(exe,'-dc',data=encoded)==data
    result=subprocess.run([exe,'-dc'],input=encoded[:12],capture_output=True,timeout=35)
    assert result.returncode != 0


def archives(name):
    Path('input').mkdir(); Path('input/text').write_bytes(b'archive content\n')
    data=bytes(range(256))*33; Path('input/binary').write_bytes(data)
    if name=='tar':
        run('/bin/tar','-cf','payload.tar','input')
        assert b'input/binary' in run('/bin/tar','-tf','payload.tar')
        Path('output').mkdir(); run('/bin/tar','-xf','payload.tar','-C','output')
    else:
        run('/usr/bin/zip','-qr','payload.zip','input')
        run('/usr/bin/unzip','-t','payload.zip')
        run('/usr/bin/unzip','-q','payload.zip','-d','output')
    assert Path('output/input/text').read_bytes()==b'archive content\n'
    assert Path('output/input/binary').read_bytes()==data


def gawk():
    script='{ sum[$1]+=$2 } END { print sum["alpha"], sum["beta"]; print gensub(/([a-z]+)-([0-9]+)/,"\\\\2:\\\\1","g","item-42") }'
    assert run('/usr/bin/gawk',script,data=b'alpha 4\nbeta 9\nalpha 5\n')==b'9 9\n42:item\n'
    Path('data').write_text('a:b\nc:d\n')
    assert run('/usr/bin/gawk','-F:', '{ print NR, $2 }','data')==b'1 b\n2 d\n'


def diffutils():
    Path('old').write_text('a\nb\n'); Path('new').write_text('a\nc\n')
    delta=run('/usr/bin/diff','-u','old','new',status=1)
    assert b'-b\n+c\n' in delta
    run('/usr/bin/cmp','old','old')
    run('/usr/bin/cmp','old','new',status=1)
    assert run('/usr/bin/diff','old','old')==b''


def search(name):
    Path('tree/sub').mkdir(parents=True)
    Path('tree/a.txt').write_text('needle one\nother\nneedle two\n')
    Path('tree/sub/b.txt').write_text('other\n')
    Path('tree/c.bin').write_bytes(b'\0binary')
    if name=='ripgrep':
        assert run('/usr/bin/rg','-n','--no-heading','needle','tree/a.txt')==b'1:needle one\n3:needle two\n'
        assert run('/usr/bin/rg','--count','needle','tree/a.txt')==b'2\n'
        run('/usr/bin/rg','absent-pattern','tree',status=1)
    else:
        found=run('/usr/bin/fdfind','--type','f','--extension','txt','.','tree').decode().splitlines()
        assert sorted(found)==['tree/a.txt','tree/sub/b.txt'],found


def tree():
    Path('branch').mkdir(); Path('branch/file').write_text('value')
    data=json.loads(run('/usr/bin/tree','-J','branch'))
    assert data[0]['name']=='branch' and data[0]['contents'][0]['name']=='file'


def bc():
    assert run('/usr/bin/bc','-l',data=b'scale=12; 1/8\n2^40\n').split()==[b'.125000000000',b'1099511627776']


def vim():
    Path('input').write_text('alpha beta\nalpha gamma\n')
    run('/usr/bin/vim.tiny','-Nu','NONE','-n','-es','input','-c','%s/alpha/delta/g','-c','wq')
    assert Path('input').read_text()=='delta beta\ndelta gamma\n'


def gio():
    Path('input').write_bytes(b'GIO-owned copy\n')
    run('/usr/bin/gio','copy','input','copy')
    assert run('/usr/bin/gio','cat','copy')==b'GIO-owned copy\n'
    run('/usr/bin/gio','move','copy','moved')
    assert not Path('copy').exists() and Path('moved').read_bytes()==Path('input').read_bytes()
    assert b'standard::size' in run('/usr/bin/gio','info','-a','standard::size','moved')
    run('/usr/bin/gio','remove','moved'); assert not Path('moved').exists()


def qt_core():
    program='/usr/lib/x86_64-linux-gnu/qt5/examples/corelib/serialization/savegame/savegame'
    saved=run(program)
    state=json.loads(Path('save.json').read_bytes())
    assert state['player']['name']=='Hero' and len(state['levels'])==2, state
    assert run(program,'load')==b'Loaded save for Hero using JSON...\n'+saved
    assert json.loads(Path('save.json').read_bytes())==state
    binary=run(program,'new','binary')
    binary_data=Path('save.dat').read_bytes()
    assert binary_data
    loaded=run(program,'load','binary')
    assert loaded==b'Loaded save for Hero using CBOR...\n'+binary, (loaded, binary)
    assert Path('save.dat').read_bytes()==binary_data


def dbus():
    command = """set -eu
/usr/bin/gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.ListNames
/usr/bin/gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.GetId
/usr/bin/gdbus introspect --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus
/usr/bin/python3.11 -c 'import pathlib,re,subprocess; output=subprocess.check_output(["/usr/bin/gdbus","call","--session","--dest","org.freedesktop.DBus","--object-path","/org/freedesktop/DBus","--method","org.freedesktop.DBus.GetConnectionUnixProcessID","org.freedesktop.DBus"],text=True); pid=re.search(r"uint32 ([0-9]+)",output).group(1); row=next(row for row in pathlib.Path("/proc/"+pid+"/limits").read_text().splitlines() if row.startswith("Max open files")); assert int(row.split()[3])>=65536,row; print("DBUS_NOFILE_65536_OK")'
# The bus and activated service share a disposable configuration directory.
/usr/bin/gsettings set org.gnome.desktop.interface clock-show-date false
test "$(/usr/bin/gsettings get org.gnome.desktop.interface clock-show-date)" = false
/usr/bin/gsettings set org.gnome.desktop.interface clock-show-date true
test "$(/usr/bin/gsettings get org.gnome.desktop.interface clock-show-date)" = true
"""
    output=run('/usr/bin/env', 'XDG_CONFIG_HOME='+str(Path('config').resolve()), '/usr/bin/dbus-run-session','--','/bin/sh','-ec',command)
    assert b'org.freedesktop.DBus' in output and b'ListNames' in output and b'NameOwnerChanged' in output
    assert b'DBUS_NOFILE_65536_OK' in output
    assert Path('config/dconf/user').is_file()


def atspi():
    command = r'''
import ast, subprocess
def run(*args):
    return subprocess.check_output(['/usr/bin/gdbus',*args],stderr=subprocess.PIPE,timeout=20).decode()
address=ast.literal_eval(run('call','--session','--dest','org.a11y.Bus',
    '--object-path','/org/a11y/bus','--method','org.a11y.Bus.GetAddress'))[0]
assert address.startswith('unix:'),address
target=['--address',address,'--dest','org.a11y.atspi.Registry',
        '--object-path','/org/a11y/atspi/accessible/root']
xml=run('introspect',*target,'--xml')
assert 'org.a11y.atspi.Accessible' in xml and 'GetChildren' in xml,xml
role=run('call',*target,'--method','org.a11y.atspi.Accessible.GetRoleName')
assert 'desktop frame' in role,role
children=run('call',*target,'--method','org.a11y.atspi.Accessible.GetChildren')
assert '[]' in children,children
names=run('call','--address',address,'--dest','org.freedesktop.DBus',
          '--object-path','/org/freedesktop/DBus','--method','org.freedesktop.DBus.ListNames')
assert 'org.a11y.atspi.Registry' in names,names
print('ATSPI_BUS_REGISTRY_DESKTOP_OK')
'''
    assert b'ATSPI_BUS_REGISTRY_DESKTOP_OK' in run('/usr/bin/dbus-run-session','--','/usr/bin/python3.11','-c',command)


name=sys.argv[1]
with tempfile.TemporaryDirectory(prefix='desktop-tools-') as directory:
    previous=os.getcwd(); os.chdir(directory)
    try:
        if name in ('gzip','bzip2','xz'): compressor(name)
        elif name in ('tar','zip'): archives(name)
        elif name in ('ripgrep','fd'): search(name)
        else: {'core-files':core_files,'core-text':core_text,'core-streams':core_streams,
               'hashes':hashes,'gawk':gawk,'diffutils':diffutils,'tree':tree,
               'bc':bc,'vim':vim,'gio':gio,'dbus':dbus,'atspi':atspi,'qt-core':qt_core}[name]()
    finally: os.chdir(previous)
print('DESKTOP_TOOL_'+name.upper().replace('-','_')+'_OK')
