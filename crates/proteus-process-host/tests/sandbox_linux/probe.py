import json, os, pathlib, socket, sys

def denied(action):
    try:
        action()
        return False
    except OSError:
        return True

pathlib.Path('written').write_text('yes')
s = socket.socket()
s.settimeout(.2)
host, port = sys.argv[3].rsplit(':', 1)
network_denied = denied(lambda: s.connect((host, int(port))))
s.close()
fds_clean = True
for name in os.listdir('/proc/self/fd'):
    if int(name) < 3:
        continue
    try:
        os.fstat(int(name))
    except OSError:
        continue
    fds_clean = False
print(json.dumps(dict(
    read=pathlib.Path('allowed').read_text(), write=pathlib.Path('written').read_text() == 'yes',
    secret_denied=denied(lambda: pathlib.Path(sys.argv[1]).read_bytes()),
    symlink_denied=denied(lambda: pathlib.Path('secret-link').read_bytes()),
    readonly_read=pathlib.Path(sys.argv[4]).read_text(),
    readonly_write_denied=denied(lambda: pathlib.Path(sys.argv[4]).write_text('forbidden')),
    path=os.environ.get('PATH'),
    root_write_denied=denied(lambda: pathlib.Path('/proteus-outside-grant').write_text('forbidden')),
    write_denied=denied(lambda: pathlib.Path(sys.argv[2]).write_text('forbidden')),
    network_denied=network_denied,
    env_clean='HOME' not in os.environ and os.environ.get('SANDBOX_EXPLICIT') == 'visible' and os.environ.get('LD_PRELOAD') == '/absent-proteus-test.so',
    fds_clean=fds_clean,
)))
