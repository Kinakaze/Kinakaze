"""Exercise the WebUI against a real Windows session (requires Python Playwright)."""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time

from playwright.sync_api import expect, sync_playwright


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--init', type=Path, required=True, help='Newly built init.exe')
    parser.add_argument('--dist', type=Path, required=True, help='Complete runtime distribution')
    parser.add_argument('--output', type=Path, default=Path('artifacts/webui-validation'))
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    checks = []
    with tempfile.TemporaryDirectory(prefix='kinakaze-webui-') as directory:
        temporary = Path(directory)
        entry = temporary / 'entry'
        entry.mkdir()
        shutil.copyfile(args.init.resolve(), entry / 'init.exe')
        shutil.copyfile(args.dist.resolve() / 'worker.exe', entry / 'worker.exe')
        root = temporary / 'root'
        log_path = output / 'session.log'
        with log_path.open('wb') as log:
            process = subprocess.Popen([
                str(entry / 'init.exe'), '--session-file', str(temporary / 'session.json'),
                '--root', str(root), '--dist', str(args.dist.resolve()), '--web', '127.0.0.1:0',
            ], stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                deadline = time.monotonic() + 30
                address = None
                while time.monotonic() < deadline:
                    match = re.search(r'Kinakaze WebUI: (http://127\.0\.0\.1:\d+)/', log_path.read_text(encoding='utf-8', errors='replace'))
                    if match:
                        address = match[1]
                        break
                    if process.poll() is not None:
                        raise AssertionError(log_path.read_text(encoding='utf-8', errors='replace'))
                    time.sleep(.05)
                assert address, 'init did not start WebUI'
                with sync_playwright() as playwright:
                    browser = playwright.chromium.launch()
                    context = browser.new_context(viewport={'width': 1440, 'height': 1080}, reduced_motion='reduce', color_scheme='light')
                    page = context.new_page()
                    errors = []
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    page.on('console', lambda message: errors.append(message.text) if 'Content Security Policy' in message.text else None)
                    page.goto(address)
                    expect(page.locator('#pool-status')).to_have_text('已就绪', timeout=30000)
                    expect(page.locator('#running')).to_have_text('0')
                    expect(page.locator('#rows .badge')).to_have_text('待命')
                    assert page.locator('.character-art').evaluate('(image) => image.complete && image.naturalWidth > 0')
                    page.locator('#help').click()
                    expect(page.locator('#help-dialog')).to_be_visible()
                    page.keyboard.press('Escape')
                    expect(page.locator('#help-dialog')).not_to_be_visible()
                    page.locator('#art-toggle').click()
                    expect(page.locator('.art-rail')).not_to_be_visible()
                    page.reload()
                    expect(page.locator('.art-rail')).not_to_be_visible()
                    expect(page.locator('#art-toggle')).to_have_attribute('aria-pressed', 'false')
                    page.locator('#art-toggle').click()
                    expect(page.locator('.art-rail')).to_be_visible()
                    expect(page.locator('#pool-status')).to_have_text('已就绪')
                    checks.append('embedded illustration loads, visibility preference persists, help dialog supports Escape')
                    page.screenshot(path=str(output / 'desktop-empty.png'), full_page=True)
                    checks.append('real session readiness and standby separated from running applications')

                    def snapshot():
                        response = context.request.get(address + '/api/state')
                        assert response.ok
                        return response.json()

                    def post(path, body, **headers):
                        csrf = snapshot()['csrf_token']
                        request_headers = {'Origin': address, 'Content-Type': 'application/json', 'X-Kinakaze-Token': csrf}
                        request_headers.update(headers)
                        return context.request.post(address + path, data=json.dumps(body), headers=request_headers)

                    page.locator('#try-example').click()
                    expect(page.locator('#program')).to_have_value('/bin/sleep')
                    expect(page.locator('#arguments input')).to_have_value('60')
                    page.locator('#arguments input').fill('180')
                    page.screenshot(path=str(output / 'launch-dialog.png'))
                    page.locator('#launch-submit').click()
                    expect(page.locator('#launch-dialog')).not_to_be_visible()
                    expect(page.locator('#running')).to_have_text('1', timeout=10000)
                    sleep = next(item for item in snapshot()['processes'] if item['program'] == '/bin/sleep')
                    sleep_row = page.locator('tr').filter(has=page.get_by_role('button', name='sleep', exact=True))
                    expect(sleep_row).to_be_visible()
                    checks.append('graphical launch creates a real Linux process that survives HTTP disconnect')

                    sleep_row.get_by_role('button', name='sleep', exact=True).focus()
                    page.evaluate('refresh()')
                    expect(sleep_row.get_by_role('button', name='sleep', exact=True)).to_be_focused()
                    sleep_row.get_by_role('button', name='sleep', exact=True).click()
                    expect(page.locator('#detail-dialog')).to_be_visible()
                    expect(page.locator('#detail-values')).to_contain_text(str(sleep['host_pid']))
                    page.locator('#detail-dialog').get_by_role('button', name='关闭', exact=True).click()
                    page.locator('#search').fill('no-such-program')
                    expect(page.locator('#empty-title')).to_have_text('没有匹配的进程')
                    page.locator('#empty-action').click()
                    expect(sleep_row).to_be_visible()
                    page.locator('[data-filter="standby"]').click()
                    expect(sleep_row).to_have_count(0)
                    page.locator('[data-filter="all"]').click()
                    page.locator('[data-sort="memory"]').click()
                    expect(page.locator('[data-sort="memory"]').locator('..')).to_have_attribute('aria-sort', 'descending')
                    checks.append('search, clear, standby filter, sorting, details and stable keyboard focus')

                    page.locator('#pause').click()
                    expect(page.locator('#pause')).to_have_attribute('aria-pressed', 'true')
                    page.wait_for_timeout(300)
                    previous_update = page.locator('#updated').inner_text()
                    page.wait_for_timeout(2200)
                    assert page.locator('#updated').inner_text() == previous_update
                    page.locator('#refresh').click()
                    expect(page.locator('#updated')).not_to_have_text(previous_update)
                    page.locator('#pause').click()
                    checks.append('pause stops automatic sampling while manual refresh remains available')

                    valid_launch = {'arguments': ['/bin/true'], 'cwd': '/', 'environment': None}
                    assert post('/api/launch', valid_launch, **{'X-Kinakaze-Token': ''}).status == 403
                    assert post('/api/launch', valid_launch, Origin='https://foreign.invalid').status == 403
                    assert post('/api/launch', valid_launch, **{'Content-Type': 'text/plain'}).status == 415
                    assert post('/api/launch', dict(valid_launch, arguments=['relative'])).status == 400
                    assert post('/api/terminate', {'pid': sleep['identity']['pid'], 'instance': 'stale'}).status == 409
                    standby = next(item for item in snapshot()['processes'] if item['standby'])
                    assert post('/api/terminate', {'pid': standby['identity']['pid'], 'instance': standby['instance']}).status == 409
                    checks.append('foreign origin, missing token, wrong MIME, invalid path, stale identity and standby termination rejected')

                    page.locator('#open-launch').click()
                    page.locator('#program').fill('/bin/sh')
                    page.locator('#arguments .icon-button').click()
                    script = 'printf "%s|%s|%s" "$PWD" "$CHECK" "$1" > /tmp/webui-output; exit 23'
                    for argument in ['-c', script, 'sh', 'one value']:
                        page.locator('#add-argument').click()
                        page.locator('#arguments input').last.fill(argument)
                    page.locator('.advanced summary').click()
                    page.locator('#cwd').fill('/tmp')
                    page.locator('#environment').fill('INVALID')
                    page.locator('#launch-submit').click()
                    expect(page.locator('#launch-error')).to_contain_text('NAME=VALUE')
                    page.locator('#environment').fill('CHECK=webui')
                    page.locator('#launch-submit').click()
                    expect(page.locator('#launch-dialog')).not_to_be_visible()
                    deadline = time.monotonic() + 10
                    marker = root / 'tmp/webui-output'
                    while not marker.exists() and time.monotonic() < deadline:
                        page.wait_for_timeout(100)
                    assert marker.read_text() == '/tmp|webui|one value'
                    expect(page.locator('#activity')).to_contain_text('进程已退出', timeout=10000)
                    assert 'CHECK=webui' not in json.dumps(snapshot())
                    checks.append('argument boundaries, cwd and environment reach the guest; fast exit is reported; environment is not exposed in snapshots')

                    page.locator('#refresh').click()
                    page.screenshot(path=str(output / 'desktop.png'), full_page=True)
                    page.locator('#theme').click()
                    expect(page.locator('html')).to_have_attribute('data-theme', 'dark')
                    page.screenshot(path=str(output / 'desktop-dark.png'), full_page=True)
                    page.reload()
                    expect(page.locator('html')).to_have_attribute('data-theme', 'dark')
                    expect(page.locator('#connection-text')).to_contain_text('已连接')
                    page.locator('#theme').click()

                    context.set_offline(True)
                    page.locator('#refresh').click()
                    expect(page.locator('#connection-text')).to_have_text('连接中断')
                    expect(sleep_row).to_be_visible()
                    expect(page.locator('#open-launch')).to_be_disabled()
                    expect(sleep_row.locator('.row-action')).to_be_disabled()
                    context.set_offline(False)
                    page.locator('#refresh').click()
                    expect(page.locator('#connection-text')).to_contain_text('已连接')
                    checks.append('theme persists; disconnect preserves data and disables mutations; reconnection restores controls')

                    page.set_viewport_size({'width': 390, 'height': 844})
                    page.evaluate('window.scrollTo(0, 0)')
                    page.screenshot(path=str(output / 'mobile.png'), full_page=True)
                    assert page.evaluate('document.documentElement.scrollWidth <= innerWidth'), page.evaluate("[...document.querySelectorAll('body *')].filter(e => e.getBoundingClientRect().right > innerWidth && e.getBoundingClientRect().width > 0).map(e => [e.tagName, e.className, e.getBoundingClientRect().width]).slice(0, 20)")
                    page.locator('#open-launch').click()
                    expect(page.locator('#program')).to_be_focused()
                    assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
                    page.screenshot(path=str(output / 'mobile-launch.png'))
                    page.keyboard.press('Escape')
                    expect(page.locator('#launch-dialog')).not_to_be_visible()
                    checks.append('390px mobile layout, accessible dialog focus and Escape dismissal')

                    for width in (320, 768, 1024):
                        page.set_viewport_size({'width': width, 'height': 900})
                        assert page.evaluate('document.documentElement.scrollWidth <= innerWidth'), f'overflow at {width}px'
                    page.set_viewport_size({'width': 390, 'height': 844})
                    page.locator('#art-toggle').click()
                    expect(page.locator('.art-rail')).not_to_be_visible()
                    assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
                    page.locator('#art-toggle').click()

                    page.set_viewport_size({'width': 1440, 'height': 1080})
                    sleep_row.locator('.row-action').click()
                    expect(page.locator('#cancel-terminate')).to_be_focused()
                    page.locator('#cancel-terminate').click()
                    assert any(item['instance'] == sleep['instance'] for item in snapshot()['processes'])
                    sleep_row.locator('.row-action').click()
                    page.locator('#confirm-terminate').click()
                    expect(page.locator('#terminate-dialog')).not_to_be_visible()
                    expect(sleep_row).to_have_count(0, timeout=10000)
                    assert not any(item['instance'] == sleep['instance'] for item in snapshot()['processes'])
                    checks.append('cancel preserves the process; confirmed termination is observed by the native exit watcher')
                    assert not errors, errors
                    checks.append('no JavaScript exceptions or Content Security Policy violations')
                    browser.close()
            finally:
                if process.poll() is None:
                    process.terminate()
                process.wait(timeout=10)
                # Job termination is asynchronous; wait for owned worker images
                # to be released before TemporaryDirectory removes the fixture.
                deadline = time.monotonic() + 10
                while (entry / 'worker.exe').exists():
                    try:
                        (entry / 'worker.exe').unlink()
                    except PermissionError:
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(.05)
    report = {'passed': True, 'checks': checks}
    (output / 'report.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
