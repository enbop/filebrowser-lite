"""HTTP upload regression tests against a native or WASIp2 server command.

python3 tests/http_uploads.py -- target/debug/filebrowser-lite-wasi --listen 127.0.0.1:{port}
python3 tests/http_uploads.py -- /path/to/fungi run -Scli -Stcp -Sinherit-network \
    -W max-memory-size=67108864 --dir {data}::data \
    /absolute/path/to/filebrowser-lite-wasi.wasm --listen 127.0.0.1:{port}

Commands run in a disposable directory. {data} and {port} are substituted.
Only the process created by this script is stopped.
"""

import contextlib
import hashlib
import http.client
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest


class UploadTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix="filebrowser-upload-test-")
        cls.addClassCleanup(cls.temp.cleanup)
        cls.root = Path(cls.temp.name)
        cls.data = cls.root / "data"
        cls.data.mkdir()
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            cls.port = sock.getsockname()[1]
        args = [arg.replace("{port}", str(cls.port)).replace("{data}", str(cls.data))
                for arg in SERVER_COMMAND]
        cls.log = tempfile.TemporaryFile(mode="w+")
        cls.addClassCleanup(cls.log.close)
        cls.server = subprocess.Popen(args, cwd=cls.root, stdout=cls.log, stderr=cls.log)
        cls.addClassCleanup(cls.stop_server)
        for _ in range(600):
            if cls.server.poll() is not None:
                cls.log.seek(0)
                raise RuntimeError(cls.log.read())
            try:
                with socket.create_connection(("127.0.0.1", cls.port), timeout=.1):
                    return
            except OSError:
                time.sleep(.1)
        raise TimeoutError("server failed to become ready")

    @classmethod
    def stop_server(cls):
        if cls.server.poll() is None:
            cls.server.terminate()
            try:
                cls.server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                cls.server.kill()
                cls.server.wait(timeout=10)

    def connect(self):
        sock = socket.create_connection(("127.0.0.1", self.port), timeout=5)
        self.addCleanup(sock.close)
        return sock

    def response(self, sock, expected):
        response = http.client.HTTPResponse(sock)
        response.begin()
        body = response.read()
        self.assertEqual(response.status, expected, body[:500])
        return response, body

    def wait_for(self, predicate):
        for _ in range(100):
            if predicate():
                return
            time.sleep(.02)
        self.fail("condition did not become true")

    def test_early_responses_do_not_wait_for_or_drain_bodies(self):
        (self.data / "existing.bin").write_bytes(b"original")
        (self.data / "directory").mkdir()
        cases = [
            ("GET", "/api/health", 200),
            ("GET", "/config.js", 200),
            ("GET", "/", 200),
            ("POST", "/api/resources/existing.bin", 409),
            ("PUT", "/api/resources/missing.bin", 404),
            ("PUT", "/api/resources/directory", 405),
            ("POST", "/api/resources/%2e%2e/escape", 400),
        ]
        for framing in ("Content-Length: 50331648", "Transfer-Encoding: chunked"):
            for method, path, status in cases:
                with self.subTest(method=method, path=path, framing=framing):
                    sock = self.connect()
                    # No body is sent. A final response must precede 100 Continue.
                    sock.sendall((f"{method} {path} HTTP/1.1\r\nHost: localhost\r\n"
                                  f"{framing}\r\nExpect: 100-continue\r\n\r\n").encode())
                    response, _ = self.response(sock, status)
                    self.assertEqual(response.getheader("Connection"), "close")
                    self.assertEqual(sock.recv(1), b"")
                    sock.close()
        self.assertEqual((self.data / "existing.bin").read_bytes(), b"original")

    def test_unused_body_cannot_be_reused_as_another_request(self):
        sock = self.connect()
        embedded = (b"POST /api/resources/unwanted.bin HTTP/1.1\r\nHost: localhost\r\n"
                    b"Content-Length: 4\r\n\r\noops")
        sock.sendall((f"GET /api/health HTTP/1.1\r\nHost: localhost\r\n"
                      f"Content-Length: {len(embedded)}\r\n\r\n").encode() + embedded)
        response, _ = self.response(sock, 200)
        self.assertEqual(response.getheader("Connection"), "close")
        self.assertFalse((self.data / "unwanted.bin").exists())

    def test_overwrite_preserves_inode_and_permissions(self):
        target = self.data / "linked.bin"
        target.write_bytes(b"original")
        target.chmod(0o600)
        alias = self.data / "alias.bin"
        os.link(target, alias)
        original_stat = target.stat()
        sock = self.connect()
        sock.sendall(b"PUT /api/resources/linked.bin HTTP/1.1\r\nHost: localhost\r\n"
                     b"Content-Length: 7\r\n\r\nupdated")
        self.response(sock, 200)
        self.assertEqual(alias.read_bytes(), b"updated")
        self.assertEqual(target.stat().st_ino, original_stat.st_ino)
        self.assertEqual(target.stat().st_mode, original_stat.st_mode)

    def test_chunked_upload_trailers_and_keep_alive(self):
        sock = self.connect()
        sock.sendall(b"POST /api/resources/chunked.bin HTTP/1.1\r\nHost: localhost\r\n"
                     b"Transfer-Encoding: chunked\r\nTrailer: X-Test\r\n\r\n"
                     b"3\r\nabc\r\n3\r\ndef\r\n0\r\nX-Test: done\r\n\r\n")
        response, _ = self.response(sock, 200)
        self.assertNotEqual(response.getheader("Connection"), "close")
        self.assertEqual((self.data / "chunked.bin").read_bytes(), b"abcdef")
        sock.sendall(b"PUT /api/resources/chunked.bin HTTP/1.1\r\nHost: localhost\r\n"
                     b"Content-Length: 0\r\n\r\n")
        self.response(sock, 200)
        self.assertEqual((self.data / "chunked.bin").read_bytes(), b"")
        sock.sendall(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\n\r\n")
        self.response(sock, 200)

    def test_interrupted_upload_preserves_destination_and_cleans_staging(self):
        for method, path in (("POST", "interrupted-new.bin"), ("PUT", "interrupted-old.bin")):
            with self.subTest(method=method):
                target = self.data / path
                if method == "PUT":
                    target.write_bytes(b"original")
                sock = self.connect()
                sock.sendall((f"{method} /api/resources/{path} HTTP/1.1\r\nHost: localhost\r\n"
                              "Content-Length: 1000000\r\n\r\npartial").encode())
                self.wait_for(lambda: any(p.stat().st_size for p in self.data.glob(".filebrowser-upload-*.tmp")))
                if method == "PUT":
                    self.assertEqual(target.read_bytes(), b"original")
                else:
                    self.assertFalse(target.exists())
                sock.shutdown(socket.SHUT_WR)
                self.response(sock, 400)
                sock.close()
                self.wait_for(lambda: not list(self.data.glob(".filebrowser-upload-*.tmp")))
                if method == "PUT":
                    self.assertEqual(target.read_bytes(), b"original")
                else:
                    self.assertFalse(target.exists())

    def test_malformed_chunk_does_not_replace_existing_file(self):
        target = self.data / "malformed.bin"
        target.write_bytes(b"original")
        sock = self.connect()
        sock.sendall(b"POST /api/resources/malformed.bin?override=true HTTP/1.1\r\n"
                     b"Host: localhost\r\nTransfer-Encoding: chunked\r\n\r\n"
                     b"3\r\nabc\r\nnot-hex\r\n")
        self.response(sock, 400)
        self.assertEqual(target.read_bytes(), b"original")
        self.wait_for(lambda: not list(self.data.glob(".filebrowser-upload-*.tmp")))

    def test_concurrent_create_does_not_silently_overwrite(self):
        first = self.connect()
        first.sendall(b"POST /api/resources/race.bin HTTP/1.1\r\nHost: localhost\r\n"
                      b"Content-Length: 6\r\n\r\nfir")
        self.wait_for(lambda: any(p.stat().st_size for p in self.data.glob(".filebrowser-upload-*.tmp")))
        with contextlib.closing(http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)) as conn:
            conn.request("POST", "/api/resources/race.bin", b"second")
            response = conn.getresponse()
            response.read()
            self.assertEqual(response.status, 200)
        first.sendall(b"st!")
        self.response(first, 409)
        self.assertEqual((self.data / "race.bin").read_bytes(), b"second")
        self.assertFalse(list(self.data.glob(".filebrowser-upload-*.tmp")))

    def test_large_text_upload_uses_bounded_buffers_and_metadata_response(self):
        # 48 MiB under a 64 MiB guest ceiling failed with the old collect() path.
        block = b"a" * 65536
        digest = hashlib.sha256()
        sock = self.connect()
        sock.sendall(b"POST /api/resources/large.txt HTTP/1.1\r\nHost: localhost\r\n"
                     b"Content-Length: 50331648\r\n\r\n")
        for _ in range(768):
            sock.sendall(block)
            digest.update(block)
        _, body = self.response(sock, 200)
        self.assertLess(len(body), 4096)
        self.assertNotIn(b'"content"', body)
        with (self.data / "large.txt").open("rb") as uploaded:
            self.assertEqual(hashlib.file_digest(uploaded, "sha256").digest(), digest.digest())
        sock.sendall(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\n\r\n")
        self.response(sock, 200)


if __name__ == "__main__":
    if "--" not in sys.argv:
        raise SystemExit(__doc__)
    SERVER_COMMAND = sys.argv[sys.argv.index("--") + 1:]
    # The executable and local component path must survive the temporary cwd.
    SERVER_COMMAND = [str(Path(arg).resolve()) if os.path.exists(arg) else arg for arg in SERVER_COMMAND]
    unittest.main(argv=[sys.argv[0]], verbosity=2)
