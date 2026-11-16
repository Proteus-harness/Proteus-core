"""Model bridge policy, opaque relay and environment/cleanup boundaries."""
import base64
import os
from pathlib import Path
import socket
import socketserver
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import patch

import polyglot_network as net

ARTIFACTS = Path(__file__).resolve().parents[3]/'target/performance/polyglot-network-tests'


class NetworkTests(unittest.TestCase):
    def setUp(self):
        ARTIFACTS.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=ARTIFACTS)
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)/'bridge.sock'

    def request(self, target, headers=''):
        connection = net.connect_unix(self.path, timeout=5)
        connection.sendall(f'CONNECT {target} HTTP/1.1\r\n{headers}\r\n'.encode())
        return connection

    def test_forbidden_destinations_are_rejected_before_upstream(self):
        with net.ModelNetwork(self.path, upstream='') as bridge, \
             patch.object(bridge, '_connect') as connect:
            for target in ('raw.githubusercontent.com:443', 'opencode.ai.evil.test:443',
                           'user@opencode.ai:443', '127.0.0.1:443', 'opencode.ai:80',
                           'https://opencode.ai:443'):
                with self.request(target) as client:
                    head, _ = net.read_header(client)
                    self.assertTrue(head.startswith(b'HTTP/1.1 403'))
            connect.assert_not_called()
            self.assertEqual(bridge.snapshot()['denied'], 6)

    def test_allowed_connection_relays_opaque_tls_bytes_and_removes_socket(self):
        def connect(host):
            self.assertEqual(host, 'opencode.ai')
            left, right = socket.socketpair()
            def echo():
                with right:
                    while block := right.recv(65536):
                        right.sendall(block)
            thread = threading.Thread(target=echo, daemon=True)
            thread.start()
            return left, b''
        with net.ModelNetwork(self.path, upstream='') as bridge, \
             patch.object(bridge, '_connect', side_effect=connect):
            self.assertEqual(self.path.stat().st_mode & 0o777, 0o600)
            with self.request('opencode.ai:443') as client:
                head, _ = net.read_header(client)
                self.assertTrue(head.startswith(b'HTTP/1.1 200'))
                payload = b'\x16\x03\x01\x00opaque TLS\x00' + os.urandom(4096)
                client.sendall(payload)
                response = b''
                while len(response) < len(payload):
                    response += client.recv(8192)
                self.assertEqual(response, payload)
            self.assertEqual(bridge.snapshot()['destinations'], {'opencode.ai': 1})
        self.assertFalse(self.path.exists())

    def test_upstream_proxy_auth_is_ours_and_client_headers_are_not_forwarded(self):
        received = []
        class Proxy(socketserver.BaseRequestHandler):
            def handle(self):
                head, _ = net.read_header(self.request)
                received.append(head)
                self.request.sendall(b'HTTP/1.1 200 Connection Established\r\n\r\n')
                while block := self.request.recv(65536):
                    self.request.sendall(block)
        with net.ThreadedTCP(('127.0.0.1', 0), Proxy) as upstream:
            thread = threading.Thread(target=upstream.serve_forever, kwargs={'poll_interval': .05}, daemon=True)
            thread.start()
            try:
                url = f'http://test-user:test-pass@127.0.0.1:{upstream.server_address[1]}'
                with net.ModelNetwork(self.path, upstream=url):
                    with self.request('models.opencode.ai:443', 'Proxy-Authorization: client-secret\r\n') as client:
                        self.assertTrue(net.read_header(client)[0].startswith(b'HTTP/1.1 200'))
                        client.sendall(b'opaque')
                        self.assertEqual(client.recv(6), b'opaque')
                self.assertEqual(len(received), 1)
                self.assertIn(b'CONNECT models.opencode.ai:443 HTTP/1.1', received[0])
                self.assertIn(b'Basic ' + base64.b64encode(b'test-user:test-pass'), received[0])
                self.assertNotIn(b'client-secret', received[0])
            finally:
                upstream.shutdown()
                thread.join(timeout=5)

    def test_inside_env_preserves_private_state_and_bypasses_proxy_for_local_rpc(self):
        original = {'HOME': '/state/home', 'XDG_CONFIG_HOME': '/state/home/.config',
                    'PATH': '/runtime/bin:/usr/bin', 'HTTPS_PROXY': 'http://old:9'}
        environment = net.inside_environment(9876, original)
        for key in ('HOME', 'XDG_CONFIG_HOME', 'PATH'):
            self.assertEqual(environment[key], original[key])
        for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy'):
            self.assertEqual(environment[key], 'http://127.0.0.1:9876')
        self.assertEqual(environment['NO_PROXY'], 'localhost,127.0.0.1')
        self.assertEqual(original['HTTPS_PROXY'], 'http://old:9')

    def test_inside_entry_executes_native_command_with_proxy_and_preserved_home(self):
        script = ('import os,sys; assert os.environ["HOME"]=="/state/private"; '
                  'assert os.environ["HTTPS_PROXY"].startswith("http://127.0.0.1:"); sys.exit(7)')
        with patch.dict(os.environ, {'HOME': '/state/private'}):
            code = net.inside(self.path, ['/usr/bin/python3', '-c', script])
        self.assertEqual(code, 7)

    def test_documented_cli_places_socket_option_after_inside(self):
        result = subprocess.run(['/usr/bin/python3', str(Path(net.__file__)), 'inside',
                                 '--socket', str(self.path), '--', '/usr/bin/true'],
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
