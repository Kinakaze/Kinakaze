"""Authenticated, bounded native init control frames (Windows named pipes)."""
import json
import struct
import time


class Controller:
    def __init__(self, endpoint, token, child, deadline):
        while True:
            try:
                self.pipe = open(endpoint, 'r+b', buffering=0)
                break
            except OSError:
                if child.process.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError('init exited or did not open its control pipe')
                time.sleep(0.005)
        self.sequence = 0
        try:
            self.call({'Hello': dict(version=1, token=token, role='Controller', adoption_ticket=None)})
        except BaseException:
            self.pipe.close()
            raise

    def exact(self, length):
        result = bytearray()
        while len(result) < length:
            part = self.pipe.read(length - len(result))
            if not part:
                raise RuntimeError('init closed the control pipe')
            result.extend(part)
        return result

    def call(self, request):
        self.sequence += 1
        data = json.dumps(dict(id=self.sequence, request=request)).encode('utf-8')
        if len(data) > 65536:
            raise ValueError('control frame too large')
        pending = memoryview(struct.pack('<I', len(data)) + data)
        while pending:
            written = self.pipe.write(pending)
            if not written:
                raise RuntimeError('init closed the control pipe while writing')
            pending = pending[written:]
        length, = struct.unpack('<I', self.exact(4))
        if not 0 < length <= 65536:
            raise RuntimeError('invalid control response size')
        reply = json.loads(self.exact(length))
        if reply['id'] != self.sequence or 'Err' in reply['result']:
            raise RuntimeError(f'control request failed: {reply}')
        return reply['result']['Ok']

