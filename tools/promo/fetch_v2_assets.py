"""Obtain the selected composer's background track from its official download form."""
from pathlib import Path
import hashlib
import json
import re
import requests

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'artifacts/promo-v2'
s = requests.Session()
url = 'https://opentracks.com/bgm/detail/12983/download'
r = s.get(url, timeout=40); r.raise_for_status()
token = re.search(r'name="csrfmiddlewaretoken" value="([^"]+)"', r.text)[1]
r = s.post(url, data={'csrfmiddlewaretoken':token, 'track':'1'}, headers={'Referer':url}, timeout=60)
r.raise_for_status()
path=OUT/'assets/summer-triangle.mp3'
if 'audio' not in r.headers.get('Content-Type','') and len(r.content)<100000:
    (OUT/'reports/music-response.html').write_text(r.text,encoding='utf-8')
    raise RuntimeError('Download form did not return audio: '+r.url)
path.write_bytes(r.content)
report=dict(title='SUMMER TRIANGLE',composer='しゃろう / Sharou',source=url,
            license='https://opentracks.com/help/articles/license/',
            profile='https://opentracks.com/creator/detail/101',
            usage='Background music synchronized with original project promotional video',
            bytes=len(r.content),sha256=hashlib.sha256(r.content).hexdigest())
(OUT/'reports/music-source.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps(report,ensure_ascii=False))
