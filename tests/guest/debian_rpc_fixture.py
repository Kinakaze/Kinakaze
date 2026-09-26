"""Bounded loopback RPC peer for the guest ABI regression."""

import socket
import struct
import threading


class RpcPeer:
    def __enter__(self):
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.socket.bind(("127.0.0.1", 0))
        self.socket.settimeout(0.2)
        self.port = self.socket.getsockname()[1]
        self.stop = threading.Event()
        self.requests = []
        self.errors = []
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()
        return self

    def serve(self):
        while not self.stop.is_set():
            try:
                data, address = self.socket.recvfrom(65536)
            except TimeoutError:
                continue
            try:
                xid, kind, rpc_version, program, version, procedure = struct.unpack_from(
                    "!6I", data
                )
                if (kind, rpc_version, program, version, procedure) != (
                    0, 2, 0x31234567, 1, 7
                ):
                    raise ValueError("unexpected RPC header")
                offset = 24
                for _ in range(2):
                    _, length = struct.unpack_from("!2I", data, offset)
                    offset += 8 + ((length + 3) & ~3)
                value, = struct.unpack_from("!i", data, offset)
                if len(data) != offset + 4:
                    raise ValueError("unexpected RPC argument size")
                self.requests.append(value)
                self.socket.sendto(
                    struct.pack("!6Ii", xid, 1, 0, 0, 0, 0, value + 1), address
                )
            except (ValueError, struct.error) as error:
                self.errors.append(str(error))

    def __exit__(self, *_):
        self.stop.set()
        self.thread.join(timeout=2)
        self.socket.close()
