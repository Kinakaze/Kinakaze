"""Record only a specified application window, with optional window-local clicks."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import subprocess
import time
from PIL import ImageGrab
from inspect_windows import user


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--hwnd', type=int, required=True)
    ap.add_argument('--output', type=Path, required=True)
    ap.add_argument('--seconds', type=float, default=24)
    ap.add_argument('--fps', type=int, default=24)
    ap.add_argument('--continue-at', type=float)
    ap.add_argument('--desktop-demo', action='store_true')
    args = ap.parse_args()
    user.SetForegroundWindow(args.hwnd)
    time.sleep(.4)
    frame = ImageGrab.grab(window=args.hwnd).convert('RGB')
    width, height = frame.size
    args.output.parent.mkdir(parents=True, exist_ok=True)
    command = ['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-f', 'rawvideo', '-pix_fmt',
               'rgb24', '-video_size', f'{width}x{height}', '-framerate', str(args.fps), '-i', '-',
               '-an', '-c:v', 'libx264', '-preset', 'veryfast', '-crf', '18', '-pix_fmt', 'yuv420p',
               '-movflags', '+faststart', str(args.output)]
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stderr=subprocess.PIPE)
    start = time.monotonic()
    count, clicked = 0, False
    desktop_actions=[(2,60,16),(6,60,16),(10,960,1010),(16,60,16),(20,960,16),(25,1400,500)] if args.desktop_demo else []
    action_index=0
    differences = []
    previous = None
    try:
        for index in range(round(args.seconds * args.fps)):
            target = start + index / args.fps
            remaining = target - time.monotonic()
            if remaining > 0:
                time.sleep(remaining)
            elapsed = time.monotonic() - start
            if action_index<len(desktop_actions) and elapsed>=desktop_actions[action_index][0]:
                _,x,y=desktop_actions[action_index]
                if user.GetForegroundWindow()==args.hwnd:
                    point=w.POINT(x,y)
                    user.ClientToScreen(args.hwnd,c.byref(point))
                    user.SetCursorPos(point.x,point.y)
                    user.mouse_event(2,0,0,0,0)
                    time.sleep(.05)
                    user.mouse_event(4,0,0,0,0)
                action_index+=1
            if args.continue_at is not None and not clicked and elapsed >= args.continue_at:
                x, y = round(width * .5), round(height * .933)
                user.PostMessageW(args.hwnd, 0x200, 0, (y << 16) | x)
                user.PostMessageW(args.hwnd, 0x201, 1, (y << 16) | x)
                user.PostMessageW(args.hwnd, 0x202, 0, (y << 16) | x)
                clicked = True
            frame = ImageGrab.grab(window=args.hwnd).convert('RGB')
            if frame.size != (width, height):
                raise RuntimeError('Window changed size while recording')
            if index % args.fps == 0:
                sample = frame.resize((160, 90)).tobytes()
                if previous is not None:
                    differences.append(sum(a != b for a, b in zip(sample, previous)) / len(sample))
                previous = sample
            process.stdin.write(frame.tobytes())
            count += 1
        process.stdin.close()
        err = process.stderr.read().decode(errors='replace')
        code = process.wait()
        if code:
            raise RuntimeError(err)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
    ImageGrab.grab(window=args.hwnd).save(args.output.with_suffix('.last.png'))
    report = dict(hwnd=args.hwnd, frames=count, fps=args.fps, size=[width,height],
                  capture_wall_seconds=round(time.monotonic()-start,3), video_seconds=count/args.fps,
                  changed_pixel_channel_fraction_per_second=differences, continue_clicked=clicked,
                  desktop_actions_scheduled=desktop_actions)
    args.output.with_suffix('.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(report))


if __name__ == '__main__':
    main()
