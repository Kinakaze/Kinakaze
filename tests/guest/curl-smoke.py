"""Exercise the actual Linux curl ELF through a V2 worker against local HTTP/TLS."""
from __future__ import annotations
from distribution import runtime_image

import argparse
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import ipaddress
import hashlib
import json
from pathlib import Path
import ssl
import subprocess
import threading

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID


BODY = b"Kinakaze V2 Linux curl response\n\x00binary\xff\n"
UPLOAD = b"actual Linux POST body\x00\xff\r\n"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def handle(self) -> None:
        try:
            super().handle()
        except (ConnectionResetError, BrokenPipeError):
            # A rejected certificate makes the client close without a request.
            pass

    def send_body(self, status: int, body: bytes, **headers: str) -> None:
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        for name, value in headers.items():
            self.send_header(name.replace("_", "-"), value)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        if self.path == "/body":
            self.send_body(200, BODY, Content_Type="application/octet-stream")
        elif self.path == "/redirect":
            self.send_body(302, b"", Location="/body")
        else:
            self.send_body(404, b"not found\n")

    def do_POST(self) -> None:
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        if self.path != "/echo" or self.headers.get("Content-Type") != "application/octet-stream":
            self.send_body(400, b"invalid request\n")
        else:
            self.send_body(200, body, Content_Type="application/octet-stream")

    def log_message(self, *_: object) -> None:
        pass


def certificates(directory: Path) -> tuple[Path, Path, Path]:
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    now = datetime.now(timezone.utc)
    ca_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Kinakaze local curl test CA")])
    ca = (x509.CertificateBuilder().subject_name(ca_name).issuer_name(ca_name)
          .public_key(key.public_key()).serial_number(x509.random_serial_number())
          .not_valid_before(now - timedelta(minutes=5)).not_valid_after(now + timedelta(days=1))
          .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
          .sign(key, hashes.SHA256()))
    leaf_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    leaf = (x509.CertificateBuilder()
            .subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")]))
            .issuer_name(ca_name).public_key(leaf_key.public_key()).serial_number(x509.random_serial_number())
            .not_valid_before(now - timedelta(minutes=5)).not_valid_after(now + timedelta(days=1))
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
            .add_extension(x509.SubjectAlternativeName([x509.DNSName("localhost"),
                           x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]), critical=False)
            .sign(key, hashes.SHA256()))
    ca_path, cert_path, key_path = directory / "ca.pem", directory / "server.pem", directory / "server.key"
    ca_path.write_bytes(ca.public_bytes(serialization.Encoding.PEM))
    cert_path.write_bytes(leaf.public_bytes(serialization.Encoding.PEM))
    key_path.write_bytes(leaf_key.private_bytes(serialization.Encoding.PEM,
                         serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
    return ca_path, cert_path, key_path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--python", action="store_true", help="also verify guest Python HTTPS and certificate rejection")
    args = parser.parse_args()
    root, dist, worker = args.root.resolve(), args.dist.resolve(), args.worker.resolve()
    fixture = root / "tmp/curl-smoke"
    fixture.mkdir(parents=True, exist_ok=True)
    (fixture / "request.bin").write_bytes(UPLOAD)
    _, certificate, private_key = certificates(fixture)
    untrusted = fixture / "untrusted"
    untrusted.mkdir(exist_ok=True)
    certificates(untrusted)
    http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    https = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(certificate, private_key)
    https.socket = context.wrap_socket(https.socket, server_side=True)
    for server in (http, https):
        threading.Thread(target=server.serve_forever, daemon=True).start()
    report: list[dict[str, object]] = []

    def run(name: str, arguments: list[str], code: int = 0, body: bytes | None = None) -> bytes:
        command = [str(worker), "run", "--root", str(root), "--dist", str(dist), "--", "/usr/bin/curl",
                   "--silent", "--show-error", "--noproxy", "*", "--connect-timeout", "5", "--max-time", "15", *arguments]
        result = subprocess.run(command, capture_output=True, timeout=45, creationflags=0x08000000)
        record = {"name": name, "exit_code": result.returncode, "expected_exit_code": code,
                  "stdout_bytes": len(result.stdout), "stderr": result.stderr.decode("utf-8", "replace"),
                  "passed": result.returncode == code and (body is None or result.stdout == body)}
        report.append(record)
        if result.returncode != code or (body is not None and result.stdout != body):
            raise AssertionError(f"{name}: {json.dumps(record)}; stdout={result.stdout!r}")
        print(f"PASS {name}", flush=True)
        return result.stdout

    try:
        version = run("Linux curl version", ["--version"])
        if not version.startswith(b"curl ") or b"SSL" not in version:
            raise AssertionError(f"curl is missing expected version/TLS support: {version!r}")
        http_url = f"http://127.0.0.1:{http.server_port}"
        https_url = f"https://127.0.0.1:{https.server_port}"
        run("HTTP binary GET", [http_url + "/body"], body=BODY)
        run("HTTP localhost resolver", [f"http://localhost:{http.server_port}/body"], body=BODY)
        run("HTTP binary POST", ["--data-binary", "@/tmp/curl-smoke/request.bin", "--header",
                                 "Content-Type: application/octet-stream", http_url + "/echo"], body=UPLOAD)
        run("HTTP redirect", ["--location", http_url + "/redirect"], body=BODY)
        run("HTTP 404 failure", ["--fail", http_url + "/missing"], code=22, body=b"")
        run("TLS trusted certificate", ["--cacert", "/tmp/curl-smoke/ca.pem",
            f"https://localhost:{https.server_port}/body"], body=BODY)
        run("TLS untrusted certificate rejected", ["--cacert", "/tmp/curl-smoke/untrusted/ca.pem",
            https_url + "/body"], code=60, body=b"")
        run("TLS hostname mismatch rejected", ["--cacert", "/tmp/curl-smoke/ca.pem", "--resolve",
            f"wrong.example:{https.server_port}:127.0.0.1", f"https://wrong.example:{https.server_port}/body"], code=60, body=b"")
        if args.python:
            source = '''import socket,ssl,sys,urllib.error,urllib.request,urllib.parse
url, trusted_ca, wrong_ca, expected = sys.argv[1:]
trusted = ssl.create_default_context(cafile=trusted_ca)
with urllib.request.urlopen(url, context=trusted, timeout=5) as response:
    assert response.read() == bytes.fromhex(expected)
try:
    urllib.request.urlopen(url, context=ssl.create_default_context(cafile=wrong_ca), timeout=5)
except urllib.error.URLError as error:
    assert isinstance(error.reason, ssl.SSLCertVerificationError), repr(error)
else:
    raise AssertionError('untrusted server accepted')
port = urllib.parse.urlsplit(url).port
with socket.create_connection(('127.0.0.1', port), timeout=5) as connection:
    try:
        trusted.wrap_socket(connection, server_hostname='wrong.example')
    except ssl.SSLCertVerificationError as error:
        assert error.verify_code != 0
    else:
        raise AssertionError('hostname mismatch accepted')
print('PYTHON_HTTPS_OK')
'''
            result = subprocess.run([str(worker), 'run', '--root', str(root), '--dist', str(dist), '--',
                                     '/usr/bin/python3', '-c', source, f'https://localhost:{https.server_port}/body',
                                     '/tmp/curl-smoke/ca.pem', '/tmp/curl-smoke/untrusted/ca.pem', BODY.hex()],
                                    capture_output=True, timeout=30, creationflags=0x08000000)
            record = {'name': 'Python HTTPS trust and hostname verification', 'exit_code': result.returncode,
                      'expected_exit_code': 0, 'stdout_bytes': len(result.stdout),
                      'stderr': result.stderr.decode('utf-8', 'replace'),
                      'source_sha256': hashlib.sha256(source.encode()).hexdigest(),
                      'runtime_sha256': hashlib.sha256((runtime_image(dist)).read_bytes()).hexdigest(),
                      'passed': result.returncode == 0 and b'PYTHON_HTTPS_OK' in result.stdout}
            report.append(record)
            if not record['passed']:
                raise AssertionError(json.dumps(record))
            print('PASS Python HTTPS trust and hostname verification', flush=True)
    finally:
        for server in (http, https):
            server.shutdown()
            server.server_close()
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
