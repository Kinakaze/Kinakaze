"""Real file/pipe codecs, sample conversion and decoded media verification."""
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import wave


def command(program, *arguments, data=None):
    result = subprocess.run(
        [program, *arguments], input=data, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, timeout=45,
    )
    assert result.returncode == 0, (program, arguments, result.returncode, result.stderr)
    return result.stdout


def ffmpeg(*arguments, data=None):
    return command('/usr/bin/ffmpeg', '-nostdin', '-hide_banner', '-loglevel', 'error',
                   '-y', *arguments, data=data)


def streams(path):
    return json.loads(command('/usr/bin/ffprobe', '-v', 'error', '-show_streams',
                              '-of', 'json', str(path)))['streams']


with tempfile.TemporaryDirectory(prefix='kinakaze-media-') as directory:
    root = Path(directory)
    samples = 4410
    pcm = b''.join(struct.pack('<hh', (i * 97 % 40001) - 20000,
                              (i * 131 % 50001) - 25000) for i in range(samples))
    original, encoded = root / 'original.wav', root / 'audio.flac'
    with wave.open(str(original), 'wb') as output:
        output.setparams((2, 2, 44100, samples, 'NONE', 'not compressed'))
        output.writeframes(pcm)
    ffmpeg('-i', str(original), '-threads', '4', '-c:a', 'flac', '-compression_level', '5',
           str(encoded))
    audio = streams(encoded)[0]
    assert (audio['codec_name'], int(audio['sample_rate']), audio['channels']) == ('flac', 44100, 2)
    assert ffmpeg('-threads', '4', '-i', str(encoded), '-f', 's16le', 'pipe:1') == pcm
    assert ffmpeg('-f', 'flac', '-i', 'pipe:0', '-f', 's16le', 'pipe:1',
                  data=encoded.read_bytes()) == pcm

    converted = root / 'resampled.wav'
    ffmpeg('-i', str(encoded), '-ar', '48000', '-ac', '1', '-c:a', 'pcm_s16le', str(converted))
    with wave.open(str(converted), 'rb') as output:
        assert (output.getnchannels(), output.getsampwidth(), output.getframerate()) == (1, 2, 48000)
        assert output.getnframes() == 4800
        assert any(output.readframes(4800))

    width, height, count = 96, 64, 6
    rgb = bytes(component for frame in range(count) for y in range(height) for x in range(width)
                for component in ((x * 3 + frame * 11) % 256, (y * 5 + frame * 17) % 256,
                                  (x + y + frame * 29) % 256))
    raw, video = root / 'frames.rgb', root / 'video.mkv'
    raw.write_bytes(rgb)
    ffmpeg('-f', 'rawvideo', '-pixel_format', 'rgb24', '-video_size', f'{width}x{height}',
           '-framerate', '12', '-i', str(raw), '-threads', '4', '-c:v', 'ffv1',
           '-level', '3', '-pix_fmt', 'bgr0', str(video))
    info = streams(video)[0]
    assert (info['codec_name'], info['width'], info['height']) == ('ffv1', width, height)
    restored = ffmpeg('-threads', '4', '-i', str(video), '-f', 'rawvideo', '-pix_fmt', 'rgb24', 'pipe:1')
    assert restored == rgb, (len(restored), len(rgb))
    print('FFMPEG_FLAC_FILE_PIPE_RESAMPLE_FFV1_PIXELS_OK')
