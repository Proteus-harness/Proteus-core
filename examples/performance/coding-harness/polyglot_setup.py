#!/usr/bin/env python3
"""Prepare public, pinned dependencies outside the source checkout or user HOME."""
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request
import zipfile

from polyglot_corpus import DATA, SOURCE, digest, manifest, write_json
from polyglot_grader import PYTHON, RUST, TOOLS

JDK_ARCHIVE = 'zulu21.54.109-ca-jdk21.0.13.0.3-linux_x64.tar.gz'
JDK_SHA = 'abe544103f1c4f0cc4b8997c0cac847eb1e842382eef57ce361c982fb0a3de11'
GRADLE_SHA = '544c35d6bd849ae8a5ed0bcea39ba677dc40f49df7d1835561582da2009b961d'
BOOST_RPM = 'boost-devel-1.90.0-7.fc44.x86_64.rpm'


def download(url, path, checksum):
    if not path.exists():
        pending = path.with_suffix(path.suffix+'.pending')
        urllib.request.urlretrieve(url, pending)
        if digest(pending) != checksum:
            raise ValueError('Unexpected downloaded archive hash: ' + url)
        pending.replace(path)
    if digest(path) != checksum:
        raise ValueError('Prepared archive changed: ' + str(path))


def setup_archives():
    TOOLS.mkdir(parents=True, exist_ok=True)
    jdk = TOOLS/JDK_ARCHIVE
    download('https://cdn.azul.com/zulu/bin/'+JDK_ARCHIVE, jdk, JDK_SHA)
    extracted = TOOLS/JDK_ARCHIVE.removesuffix('.tar.gz')
    if not extracted.exists():
        with tarfile.open(jdk) as archive:
            archive.extractall(TOOLS, filter='data')
    if not (TOOLS/'jdk').exists():
        (TOOLS/'jdk').symlink_to(extracted.name)
    if (TOOLS/'jdk').resolve() != extracted.resolve():
        raise ValueError('Prepared JDK points to a different version')
    gradle = TOOLS/'gradle-8.7-bin.zip'
    download('https://services.gradle.org/distributions/'+gradle.name, gradle, GRADLE_SHA)
    if not (TOOLS/'gradle-8.7').exists():
        with zipfile.ZipFile(gradle) as archive:
            if any(Path(p).is_absolute() or '..' in Path(p).parts for p in archive.namelist()):
                raise ValueError('Invalid archive member')
            archive.extractall(TOOLS)
    (TOOLS/'gradle-8.7/bin/gradle').chmod(0o755)


def setup_npm():
    target = TOOLS/'npm'
    target.mkdir(exist_ok=True)
    source = SOURCE/'javascript/exercises/practice/affine-cipher/package.json'
    if not (target/'package.json').exists():
        shutil.copy2(source, target/'package.json')
    if digest(source) != digest(target/'package.json'):
        raise ValueError('Shared NPM manifest differs from pinned dataset')
    if not (target/'node_modules').exists():
        command = 'ci' if (target/'package-lock.json').exists() else 'install'
        subprocess.run(['npm', command, '--ignore-scripts', '--no-audit', '--no-fund'], cwd=target, check=True)


def setup_gradle():
    init = TOOLS/'resolve.gradle'
    init.write_text("allprojects { tasks.register('resolveAll') { doLast { "
                    "configurations.findAll { it.canBeResolved }.each { it.resolve() } } } }\n")
    environment = dict(os.environ, JAVA_HOME=str((TOOLS/'jdk').resolve()),
                       GRADLE_USER_HOME=str(TOOLS/'gradle-home'))
    for name in ('affine-cipher', 'rest-api', 'hangman'):
        target = TOOLS/'java-warm'/name
        if not target.exists():
            shutil.copytree(SOURCE/'java/exercises/practice'/name, target)
        with (TOOLS/('gradle-warm-'+name+'.log')).open('w') as output:
            subprocess.run([str(TOOLS/'gradle-8.7/bin/gradle'), '--no-daemon', '--console=plain',
                            '--init-script', str(init), 'resolveAll'], cwd=target,
                           env=environment, stdout=output, stderr=subprocess.STDOUT, check=True)


def setup_cargo():
    environment = dict(os.environ, CARGO_HOME=str(TOOLS/'cargo-home'),
                       PATH=str(RUST/'bin')+':'+os.environ['PATH'])
    source = SOURCE/'rust/exercises/practice'
    specs = [(name, source/name/'Cargo.toml', False) for name in ('gigasecond', 'grep', 'simple-cipher')]
    # Some official reference examples intentionally declare a different
    # dependency set. These extra caches never alter the submitted manifest.
    specs += [(p.parent.parent.name, p, True) for p in sorted(source.glob('*/.meta/Cargo-example.toml'))]
    for name, cargo, reference in specs:
        target = TOOLS/('rust-reference-warm' if reference else 'rust-warm')/name
        if not target.exists():
            shutil.copytree(source/name, target)
        (target/'Cargo.toml').write_text(cargo.read_text()+'\n[workspace]\n')
        subprocess.run([str(RUST/'bin/cargo'), 'fetch', '--manifest-path', str(target/'Cargo.toml')],
                       env=environment, check=True)


def setup_boost():
    # Fedora host already supplies the matching shared library; the devel RPM
    # is only extracted locally. No package installation or trust-store edit.
    rpms = TOOLS/'boost-rpms'
    rpms.mkdir(exist_ok=True)
    rpm = rpms/BOOST_RPM
    if not rpm.exists():
        subprocess.run(['dnf', '--repo=fedora', '--repo=updates', 'download',
                        '--destdir', str(rpms), BOOST_RPM.removesuffix('.rpm')], check=True)
    target = TOOLS/'boost'
    if not (target/'usr/include/boost').exists():
        target.mkdir(exist_ok=True)
        unpack = subprocess.Popen(['rpm2cpio', str(rpm)], stdout=subprocess.PIPE)
        subprocess.run(['cpio', '-id', '--quiet'], cwd=target, stdin=unpack.stdout, check=True)
        unpack.stdout.close()
        if unpack.wait():
            raise ValueError('Could not extract Boost development headers')
    library = Path('/usr/lib64/libboost_date_time.so.1.90.0')
    if not library.exists():
        raise ValueError('Pinned Boost 1.90 date_time runtime is required on the host')
    shutil.copy2(library, target/'usr/lib64'/library.name)


def receipt():
    versions = {}
    commands = {'python': [str(PYTHON/'bin/python'), '--version'],
                'pytest': [str(PYTHON/'bin/python'), '-m', 'pytest', '--version'],
                'g++': ['g++', '--version'], 'cmake': ['cmake', '--version'],
                'go': ['go', 'version'], 'node': ['node', '--version'], 'npm': ['npm', '--version'],
                'rustc': [str(RUST/'bin/rustc'), '--version'],
                'cargo': [str(RUST/'bin/cargo'), '--version'],
                'jdk': [str(TOOLS/'jdk/bin/java'), '-version']}
    for key, command in commands.items():
        versions[key] = subprocess.check_output(command, stderr=subprocess.STDOUT, text=True).strip()
    result = {'format': 1, 'versions': versions, 'jdk_archive_sha256': JDK_SHA,
              'gradle_archive_sha256': GRADLE_SHA,
              'boost_rpm_sha256': digest(TOOLS/'boost-rpms'/BOOST_RPM),
              'boost_library_sha256': digest(TOOLS/'boost/usr/lib64/libboost_date_time.so.1.90.0'),
              'npm_lock_sha256': digest(TOOLS/'npm/package-lock.json'),
              'cargo_locks': {str(p.relative_to(TOOLS)): digest(p)
                              for directory in ('rust-warm', 'rust-reference-warm')
                              for p in sorted((TOOLS/directory).glob('*/Cargo.lock'))},
              'gradle_dependencies': manifest(TOOLS/'gradle-home/caches/modules-2')}
    # Save the complete lock/cache snapshot once, not once for every task.
    write_json(TOOLS/'receipt.json', result)
    return TOOLS/'receipt.json'


if __name__ == '__main__':
    setup_archives()
    setup_npm()
    setup_gradle()
    setup_cargo()
    setup_boost()
    print(receipt())
