"""Launch the isolated demo with the Debian GI Python module search path."""
from pathlib import Path
import runpy
import sys
import os

ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT/'tools'))
import importlib.util
spec=importlib.util.spec_from_file_location('promo_run_gnome',ROOT/'tools/run-gnome.py')
launcher=importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)
launcher.SESSION="import sys, os\nos.environ.update({'LANG': 'C.UTF-8', 'LC_ALL': 'C.UTF-8'})\nsys.path.insert(0, '/usr/lib/python3/dist-packages')\n"+launcher.SESSION
(ROOT/'artifacts/promo-v1/reports/desktop-owner.pid').write_text(str(os.getpid()))
sys.exit(launcher.main())
