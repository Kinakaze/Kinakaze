"""Run the existing dependency verifier with retrying Windows HTTPS transport."""
import runpy
from pathlib import Path
import sys
from fetch_desktop import download

ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT/'tools/guest-deps'))
import guest_deps
original=guest_deps.PackageCache._load


def load(self,package):
    if not (self.directory/package['filename']).exists():
        name,status=download(package['package'])
        print(name,status,flush=True)
        if status not in ('cached','downloaded'):
            raise RuntimeError(status)
    return original(self,package)


guest_deps.PackageCache._load=load
requested=sys.argv[1:] or ['gnome-shell','gnome-control-center','gnome-terminal']
sys.argv=[str(ROOT/'tools/prepare-root.py'),'--source','E:/Naka/crysoacu/target/debug',
          '--root',str(ROOT/'artifacts/promo-v1/gnome-root'),
          '--dist',str(ROOT/'artifacts/release-v0.1.0'),
          *[arg for package in requested for arg in ('--package',package)],
          '--report',str(ROOT/'artifacts/promo-v1/reports/gnome-provenance.json'),'--offline']
runpy.run_path(str(ROOT/'tools/prepare-root.py'),run_name='__main__')
