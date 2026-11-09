from hashlib import sha256
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
import os
import subprocess
import sys
import tempfile
from threading import Thread
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import download

PAYLOAD = b"verified build archive"
DIGEST = sha256(PAYLOAD).hexdigest()


class DownloadTests(unittest.TestCase):
    def test_verified_cache_avoids_network_and_invalid_cache_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "archive"
            output.write_bytes(PAYLOAD)
            with patch.object(download.subprocess, "run") as run:
                download.fetch("https://example.invalid/archive", DIGEST, output)
                run.assert_not_called()
                output.write_bytes(b"wrong")
                with self.assertRaisesRegex(ValueError, "cached archive SHA256 mismatch"):
                    download.fetch("https://example.invalid/archive", DIGEST, output)
                run.assert_not_called()

    def test_download_has_bounded_retries_and_atomically_publishes_verified_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "archive"
            def transfer(command, **kwargs):
                self.assertEqual(command[command.index("--retry") + 1], "5")
                self.assertEqual(command[command.index("--retry-max-time") + 1], "180")
                self.assertEqual(kwargs["timeout"], 210)
                self.assertTrue(kwargs["check"])
                self.assertFalse(output.exists())
                Path(command[command.index("--output") + 1]).write_bytes(PAYLOAD)
            with patch.object(download.subprocess, "run", side_effect=transfer):
                download.fetch("https://example.invalid/archive", DIGEST, output)
            self.assertEqual(output.read_bytes(), PAYLOAD)
            self.assertEqual(list(Path(directory).iterdir()), [output])

    def test_hash_failure_never_publishes_archive_and_removes_partial_download(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "archive"
            def transfer(command, **kwargs):
                Path(command[command.index("--output") + 1]).write_bytes(b"wrong")
            with patch.object(download.subprocess, "run", side_effect=transfer):
                with self.assertRaisesRegex(ValueError, "downloaded archive SHA256 mismatch"):
                    download.fetch("https://example.invalid/archive", DIGEST, output)
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_network_failure_never_publishes_or_leaves_temporary_files(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "archive"
            with patch.object(download.subprocess, "run", side_effect=subprocess.CalledProcessError(22, ["curl"])):
                with self.assertRaises(subprocess.CalledProcessError):
                    download.fetch("https://example.invalid/archive", DIGEST, output)
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_real_transient_504_retries_return_only_verified_archive(self):
        attempts = []
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                attempts.append(self.path)
                self.send_response(504 if len(attempts) < 3 else 200)
                self.end_headers()
                self.wfile.write(b"retry" if len(attempts) < 3 else PAYLOAD)

            def log_message(self, *args):
                pass

        server = HTTPServer(("127.0.0.1", 0), Handler)
        worker = Thread(target=server.serve_forever)
        worker.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / "archive"
                with patch.dict(os.environ, {"NO_PROXY": "127.0.0.1", "no_proxy": "127.0.0.1"}):
                    download.fetch(f"http://127.0.0.1:{server.server_port}/archive", DIGEST, output)
                self.assertEqual(output.read_bytes(), PAYLOAD)
                self.assertEqual(attempts, ["/archive"] * 3)
        finally:
            server.shutdown()
            worker.join()
            server.server_close()


if __name__ == "__main__":
    unittest.main()
