"""Check the packaged local player and its chapter navigation."""
from pathlib import Path
import json
from playwright.sync_api import sync_playwright

ROOT=Path(__file__).resolve().parents[3]
OUT=ROOT/'artifacts/promo-amv'
settings=json.loads((Path(__file__).parent/'amv-settings.json').read_text(encoding='utf-8'))
errors=[]
with sync_playwright() as pw:
    browser=pw.chromium.launch(headless=True,args=['--allow-file-access-from-files'])
    try:
        page=browser.new_page(viewport={'width':1440,'height':1000})
        page.on('pageerror',lambda error:errors.append(str(error)))
        page.on('console',lambda msg:errors.append(msg.text) if msg.type=='error' else None)
        page.goto((OUT/'index.html').as_uri(),wait_until='load')
        page.wait_for_function('document.getElementById("film").readyState >= 2')
        media=page.locator('video').evaluate('(v)=>({width:v.videoWidth,height:v.videoHeight,duration:v.duration,source:v.currentSrc})')
        assert [media['width'],media['height']]==[settings['width'],settings['height']],media
        assert settings['filename'] in media['source'],media
        assert abs(media['duration']-settings['frames']/settings['fps'])<.1,media
        assert '眨眼帧' not in page.locator('body').inner_text()
        visited=[]
        chapters=page.locator('button[data-time]').evaluate_all('(buttons)=>buttons.map(b=>Number(b.dataset.time))')
        for seconds in chapters:
            page.locator(f'button[data-time="{seconds:g}"]').click()
            page.wait_for_function('(t)=>{const v=document.getElementById("film");return !v.paused&&!v.seeking&&v.readyState>=2&&Math.abs(v.currentTime-t)<1}',arg=seconds)
            page.locator('video').evaluate('(v)=>v.pause()')
            visited.append(seconds)
        page.screenshot(path=str(OUT/f"reports/preview-{settings['reportSuffix']}-desktop.png"),full_page=True)
        page.set_viewport_size({'width':390,'height':844})
        page.wait_for_function('document.documentElement.scrollWidth<=window.innerWidth')
        page.screenshot(path=str(OUT/f"reports/preview-{settings['reportSuffix']}-mobile.png"),full_page=True)
        assert not errors,errors
        report={'status':'passed','media':media,'chapter_seek_seconds':visited,'mobile_width':390,'javascript_errors':errors}
        (OUT/f"reports/preview-{settings['reportSuffix']}.json").write_text(json.dumps(report,indent=2),encoding='utf-8')
        print(json.dumps(report,indent=2))
    finally:
        browser.close()
