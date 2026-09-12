"""Sequencer routing, buffer ownership, poll readiness, queues and cleanup."""
import ctypes as C
import json
import select
import struct
import time

alsa = C.CDLL('libasound.so.2')
P, I, U = C.c_void_p, C.c_int, C.c_uint
def api(name, args, result=I):
    function = getattr(alsa, 'snd_seq_' + name)
    function.argtypes, function.restype = args, result
    return function

open_ = api('open', [C.POINTER(P), C.c_char_p, I, I])
close = api('close', [P])
client_id = api('client_id', [P])
create_port = api('create_simple_port', [P, C.c_char_p, U, U])
connect = api('connect_to', [P, I, I, I])
direct = api('event_output_direct', [P, P])
output = api('event_output', [P, P])
drain = api('drain_output', [P])
pending = api('event_input_pending', [P, I])
input_ = api('event_input', [P, C.POINTER(P)])
alloc_queue = api('alloc_named_queue', [P, C.c_char_p])
free_queue = api('free_queue', [P, I])
control = api('control_queue', [P, I, I, I, P])
drop_output = api('drop_output', [P])
set_buffer = api('set_input_buffer_size', [P, C.c_size_t])
class PollFd(C.Structure):
    _fields_ = [('fd', I), ('events', C.c_short), ('revents', C.c_short)]
poll_fds = api('poll_descriptors', [P, C.POINTER(PollFd), U, C.c_short])
handles = []
started = time.perf_counter()
timings = {}
try:
    for name in (b'source', b'sink', b'other'):
        handle = P()
        assert open_(C.byref(handle), b'default', 3, 1) == 0
        handles.append(handle)
        assert api('set_client_name', [P, C.c_char_p])(handle, name) == 0
    source, sink, other = handles
    ports = [create_port(handle, b'test port', 1|2|32|64, 2) for handle in handles]
    assert min(ports) >= 0
    assert connect(source, ports[0], client_id(sink), ports[1]) == 0
    assert connect(source, ports[0], client_id(other), ports[2]) == 0
    assert connect(source, ports[0], client_id(sink), ports[1]) == -16
    timings['open_ports_subscriptions_ms'] = (time.perf_counter()-started)*1000
    info = P()
    assert api('client_info_malloc', [C.POINTER(P)])(C.byref(info)) == 0
    try:
        api('client_info_set_client', [P, I], None)(info, -1)
        seen = {}
        while api('query_next_client', [P, P])(source, info) == 0:
            seen[api('client_info_get_client', [P])(info)] = api('client_info_get_name', [P], C.c_char_p)(info)
        assert seen[client_id(source)] == b'source' and seen[client_id(sink)] == b'sink'
        timings['enumeration_done_ms'] = (time.perf_counter()-started)*1000
    finally:
        api('client_info_free', [P], None)(info)
    descriptor = PollFd()
    assert poll_fds(sink, C.byref(descriptor), 1, 1) == 1
    fd = descriptor.fd
    def ready(timeout=0):
        return bool(select.select([fd], [], [], timeout)[0])
    def event(kind=6, payload=None):
        data = bytearray(28)
        data[0], data[3], data[13], data[14] = kind, 253, ports[0], 254
        data[16:19] = bytes((0, 60, 99))
        if payload is not None:
            data[1] = 4
            struct.pack_into('=IQ', data, 16, len(payload), C.addressof(payload))
        return (C.c_ubyte * 28).from_buffer_copy(data)
    def receive(handle):
        pointer = P()
        result = input_(handle, C.byref(pointer))
        assert result >= 0 and pointer.value, result
        data = C.string_at(pointer, 28)
        payload = b''
        if data[1] & 12:
            length, address = struct.unpack_from('=IQ', data, 16)
            payload = C.string_at(address, length)
        return data, payload
    assert not ready()
    assert output(source, event()) == 28
    assert pending(sink, 1) == 0 and not ready()
    assert drain(source) == 0 and ready()
    data, _ = receive(sink)
    assert data[12:16] == bytes((client_id(source), ports[0], client_id(sink), ports[1]))
    receive(other)
    assert not ready()
    payload = (C.c_ubyte * 5)(0xf0, 1, 2, 3, 0xf7)
    assert output(source, event(130, payload)) == 33
    payload[1] = 77
    assert drain(source) == 0
    for handle in (sink, other):
        assert receive(handle)[1] == b'\xf0\x01\x02\x03\xf7'
    queue = alloc_queue(source, b'clock')
    assert queue >= 0
    scheduled = event()
    scheduled[3], scheduled[1] = queue, 1
    C.memmove(C.addressof(scheduled)+4, struct.pack('=II', 0, 40_000_000), 8)
    assert direct(source, scheduled) == 28
    assert control(source, queue, 30, 0, None) >= 0
    # Queue controls themselves remain buffered until drained.
    assert not ready(0.06)
    started = time.monotonic()
    assert drain(source) == 0
    assert ready(2) and time.monotonic()-started >= 0.02
    receive(sink); receive(other)
    assert control(source, queue, 32, 0, None) >= 0 and drain(source) == 0
    scheduled[1] = 3  # realtime relative to stopped position
    assert direct(source, scheduled) == 28 and not ready(0.06)
    assert control(source, queue, 31, 0, None) >= 0 and drain(source) == 0
    assert ready(2)
    receive(sink); receive(other)
    assert free_queue(source, queue) == 0
    assert direct(source, scheduled) == -22
    assert set_buffer(sink, 28) == 0
    assert direct(source, event()) == 28
    assert direct(source, event()) == 28  # other subscriber still accepts it
    assert direct(source, event()) == 28  # retain readiness after overflow report
    pointer = P()
    assert input_(sink, C.byref(pointer)) == -28 and not pointer.value
    assert ready()
    receive(sink)
    assert not ready()
    for _ in range(3): receive(other)
    assert input_(sink, C.byref(pointer)) == -11 and not pointer.value
    assert drop_output(source) == 0
    timings['routing_queues_done_ms'] = (time.perf_counter()-started)*1000
finally:
    for handle in reversed(handles):
        assert close(handle) == 0
timings['close_done_ms'] = (time.perf_counter()-started)*1000
print(json.dumps(timings), flush=True)
print('AlsaSequencerProbe: PASS', flush=True)
