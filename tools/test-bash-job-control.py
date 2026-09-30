"""Check Bash pipelines, unreaped group leaders and foreground interruption."""
import argparse
import json
from pathlib import Path
import shutil
import uuid
from init_pool import InitPool, distribution_hashes

parser = argparse.ArgumentParser(description=__doc__)
for name in ('root', 'dist', 'output'):
    parser.add_argument('--' + name, type=Path, required=True)
args = parser.parse_args()
root, dist, output = (p.resolve() for p in (args.root, args.dist, args.output))
output.mkdir(parents=True, exist_ok=False)
guest = '/tmp/bash-job-control-' + uuid.uuid4().hex
stage = root / guest.lstrip('/')
stage.mkdir()
shutil.copy2(Path(__file__).resolve().parents[1] / 'tests/guest/BashProcessGroupProbe.py', stage/'probe.py')
report = dict(guest=guest, sha256=distribution_hashes(dist), passed=False)
try:
    with InitPool(root, dist, output/'session', timeout=150, size=1) as pool:
        report['run'] = pool.run(['/usr/bin/python3', guest+'/probe.py'], cwd=guest,
            environment=['PATH=/usr/bin:/bin', 'LC_ALL=C'], expect=['BASH_PROCESS_GROUPS_OK'])
        report['passed'] = report['run']['status'] == 'passed'
except Exception as error:
    report['error'] = repr(error)
finally:
    if (stage/'bash-process-groups.txt').is_file():
        shutil.copy2(stage/'bash-process-groups.txt', output/'terminal.txt')
    (output/'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
print(json.dumps(dict(passed=report['passed'], error=report.get('error'))), flush=True)
raise SystemExit(int(not report['passed']))
