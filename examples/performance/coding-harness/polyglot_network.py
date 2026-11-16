#!/usr/bin/env python3
"""Model-only CONNECT bridge across a benchmark's isolated network namespace.

TLS remains opaque. The bridge neither records HTTP payloads nor forwards
client-supplied proxy headers. Task tools cannot reach public test/solution hosts.
"""
import argparse
import base64
import os
from pathlib import Path
import re
import select
import socket
import socketserver
import ssl
import subprocess
import sys
import threading
from urllib.parse import unquote, urlsplit

ALLOWED_HOSTS = frozenset(('opencode.ai', 'models.opencode.ai'))
MAX_HEADER = 16384


def connect_unix(path, timeout=30):
    """Use a short proc-fd path; benchmark result directories exceed sun_path."""
    path = Path(path)
    descriptor = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        connection.settimeout(timeout)
        connection.connect(f'/proc/self/fd/{descriptor}/{path.name}')
        return connection
    except BaseException:
        connection.close()
        raise
    finally:
        os.close(descriptor)


def read_header(connection):
    data = b''
    while b'\r\n\r\n' not in data:
        block = connection.recv(4096)
        if not block:
            raise ValueError('Incomplete proxy handshake')
        data += block
        if len(data) > MAX_HEADER:
            raise ValueError('Proxy handshake exceeds the limit')
    head, remainder = data.split(b'\r\n\r\n', 1)
    return head, remainder


def relay(left, right, record=None):
    """Forward opaque bytes with backpressure and preserve half-close semantics."""
    peers = {left: right, right: left}
    while peers:
        ready, _, _ = select.select(list(peers), [], [], 1800)
        if not ready:
            return
        for source in ready:
            block = source.recv(65536)
            if not block:
                destination = peers.pop(source)
                destination.shutdown(socket.SHUT_WR)
                continue
            peers[source].sendall(block)
            if record:
                record('client_bytes' if source is left else 'upstream_bytes', len(block))


class ThreadedUnix(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
    block_on_close = False

    def handle_error(self, request, client_address):
        pass  # Exception text can include sensitive proxy URL information.


class ThreadedTCP(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = True
    block_on_close = False
    allow_reuse_address = True

    def handle_error(self, request, client_address):
        pass


class ModelNetwork:
    """Host-side bridge; an explicit empty upstream selects direct connections."""

    def __init__(self, socketpath, *, upstream=None):
        self.path = Path(socketpath)
        if upstream is None:
            upstream = next((os.environ[name] for name in
                             ('HTTPS_PROXY', 'https_proxy', 'HTTP_PROXY', 'http_proxy')
                             if os.environ.get(name)), '')
        self.upstream = urlsplit(upstream) if upstream else None
        if self.upstream and (self.upstream.scheme not in ('http', 'https')
                              or not self.upstream.hostname):
            raise ValueError('Model bridge requires an HTTP(S) upstream proxy')
        self.lock = threading.Lock()
        self.connections = set()
        self.counters = dict(accepted=0, denied=0, failures=0,
                             client_bytes=0, upstream_bytes=0)
        self.destinations = {}
        self.server = None

    def record(self, key, amount=1):
        with self.lock:
            self.counters[key] += amount

    def snapshot(self):
        with self.lock:
            return {**self.counters, 'destinations': dict(self.destinations)}

    def _connect(self, host):
        if not self.upstream:
            return socket.create_connection((host, 443), timeout=30), b''
        proxy = self.upstream
        port = proxy.port or (443 if proxy.scheme == 'https' else 80)
        connection = socket.create_connection((proxy.hostname, port), timeout=30)
        try:
            if proxy.scheme == 'https':
                connection = ssl.create_default_context().wrap_socket(connection, server_hostname=proxy.hostname)
            headers = [f'CONNECT {host}:443 HTTP/1.1', f'Host: {host}:443']
            if proxy.username is not None:
                credential = unquote(proxy.username) + ':' + unquote(proxy.password or '')
                headers.append('Proxy-Authorization: Basic ' + base64.b64encode(credential.encode()).decode())
            connection.sendall(('\r\n'.join(headers) + '\r\n\r\n').encode('ascii'))
            head, tail = read_header(connection)
            if not re.match(rb'HTTP/1\.[01] 200(?: |$)', head.split(b'\r\n', 1)[0]):
                raise ValueError('Upstream proxy rejected model connection')
            return connection, tail
        except BaseException:
            connection.close()
            raise

    def __enter__(self):
        owner = self

        class Handler(socketserver.BaseRequestHandler):
            def handle(self):
                remote = None
                established = False
                try:
                    self.request.settimeout(30)
                    head, tail = read_header(self.request)
                    first = head.split(b'\r\n', 1)[0].decode('ascii')
                    parts = first.split(' ')
                    match = re.fullmatch(r'([A-Za-z0-9.-]+):443', parts[1]) if len(parts) == 3 else None
                    if (parts[0] != 'CONNECT' or not match or parts[2] not in ('HTTP/1.0', 'HTTP/1.1')
                            or match[1].lower() not in ALLOWED_HOSTS):
                        owner.record('denied')
                        self.request.sendall(b'HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n')
                        return
                    host = match[1].lower()
                    remote, upstream_tail = owner._connect(host)
                    with owner.lock:
                        owner.connections.update((self.request, remote))
                        owner.counters['accepted'] += 1
                        owner.destinations[host] = owner.destinations.get(host, 0) + 1
                    self.request.sendall(b'HTTP/1.1 200 Connection Established\r\n\r\n' + upstream_tail)
                    established = True
                    if tail:
                        remote.sendall(tail)
                        owner.record('client_bytes', len(tail))
                    relay(self.request, remote, owner.record)
                except (OSError, ValueError, UnicodeError):
                    owner.record('failures')
                    if not established:
                        try:
                            self.request.sendall(b'HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n')
                        except OSError:
                            pass
                finally:
                    with owner.lock:
                        owner.connections.discard(self.request)
                        owner.connections.discard(remote)
                    if remote:
                        remote.close()

        self.path.parent.mkdir(parents=True, exist_ok=True)
        # Never unlink an unknown existing socket: it may belong to a live run.
        descriptor = os.open(self.path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            self.server = ThreadedUnix(f'/proc/self/fd/{descriptor}/{self.path.name}', Handler)
        finally:
            os.close(descriptor)
        os.chmod(self.path, 0o600)
        self.thread = threading.Thread(target=self.server.serve_forever, kwargs={'poll_interval': .1}, daemon=True)
        self.thread.start()
        return self

    def __exit__(self, *unused):
        if self.server:
            self.server.shutdown()
            with self.lock:
                connections = list(self.connections)
            for connection in connections:
                try:
                    connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
            self.server.server_close()
            self.thread.join(timeout=5)
            self.path.unlink(missing_ok=True)


def inside_environment(port, base=None):
    environment = dict(os.environ if base is None else base)
    proxy = f'http://127.0.0.1:{port}'
    for name in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy'):
        environment[name] = proxy
    environment['NO_PROXY'] = environment['no_proxy'] = 'localhost,127.0.0.1'
    environment['NODE_USE_ENV_PROXY'] = '1'
    return environment


def inside(socketpath, command):
    class Handler(socketserver.BaseRequestHandler):
        def handle(self):
            try:
                with connect_unix(socketpath) as remote:
                    relay(self.request, remote)
            except OSError:
                pass
    with ThreadedTCP(('127.0.0.1', 0), Handler) as server:
        thread = threading.Thread(target=server.serve_forever, kwargs={'poll_interval': .1}, daemon=True)
        thread.start()
        try:
            return subprocess.run(command, env=inside_environment(server.server_address[1]),
                                  stdin=subprocess.DEVNULL).returncode
        finally:
            server.shutdown()
            thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['inside'])
    parser.add_argument('--socket', type=Path, required=True)
    argv = sys.argv[1:]
    separator = argv.index('--') if '--' in argv else len(argv)
    args = parser.parse_args(argv[:separator])
    command = argv[separator+1:]
    if not command:
        parser.error('native command is required after --')
    return inside(args.socket, command)


if __name__ == '__main__':
    raise SystemExit(main())
