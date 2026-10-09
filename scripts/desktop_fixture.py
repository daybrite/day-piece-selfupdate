#!/usr/bin/env python3
"""Exercise the real unprivileged Linux/Windows helper with signed synthetic releases."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
WINDOWS = sys.platform == 'win32'


def run(*args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


def wait(predicate):
    until = time.monotonic() + 30
    while time.monotonic() < until:
        if predicate(): return
        time.sleep(.05)
    raise TimeoutError('fixture did not complete')


def main():
    if sys.platform not in ('win32', 'linux'):
        raise SystemExit('Run this fixture on Windows or Linux; use prototype.py on macOS')
    run('cargo', 'build', '-p', 'day-selfupdate-tools', cwd=ROOT)
    tool = ROOT / 'target/debug' / ('day-selfupdate-tool.exe' if WINDOWS else 'day-selfupdate-tool')
    (ROOT / 'build').mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='desktop-fixture-', dir=ROOT / 'build') as temp:
        base = Path(temp).resolve()
        inbox = base / 'inbox'; inbox.mkdir()
        key = base / 'fixture.key'
        public = run(tool, 'keygen', key, capture_output=True, text=True).stdout.strip()
        marker = base / 'relaunched'
        if WINDOWS:
            destination = base / 'installed app'; destination.mkdir()
            executable = destination / 'fixture.exe'
            source = base / 'fixture.rs'
            source.write_text('fn main() { if std::env::args().any(|a| a == "--caller") { let p = std::env::var("DYSU_FIXTURE_READY").unwrap(); while !std::path::Path::new(&p).exists() { std::thread::sleep(std::time::Duration::from_millis(50)); } return; } std::fs::write(std::env::var("DYSU_FIXTURE_MARKER").unwrap(), b"new").unwrap(); }')
            new = base / 'new.exe'
            run('rustc', source, '-o', new)
            shutil.copy2(new, executable)
            payload = inbox / 'update.exe'
            nsi = base / 'fixture.nsi'
            nsi.write_text(f'Unicode true\nRequestExecutionLevel user\nOutFile "{payload}"\nInstallDir "{destination}"\nSection\nSetOutPath "$INSTDIR"\nFile /oname=fixture.exe "{new}"\nSectionEnd\n')
            compiler = shutil.which('makensis') or str(Path(os.environ.get('ProgramFiles(x86)', 'C:/Program Files (x86)')) / 'NSIS/makensis.exe')
            run(compiler, '/V2', nsi)
        else:
            destination = base / 'installed.appimage'
            executable = destination
            destination.write_text('#!/bin/sh\nexit 0\n'); destination.chmod(0o700)
            payload = inbox / 'update.appimage'
            payload.write_text('#!/bin/sh\nprintf new > "$DYSU_FIXTURE_MARKER"\n')
        # Windows on ARM reports 'ARM64' and macOS 'arm64'; match Rust's std::env::consts::ARCH.
        arch = {'amd64': 'x86_64', 'arm64': 'aarch64'}.get(platform.machine().lower(), platform.machine())
        target = ('windows-winui-' if WINDOWS else 'linux-gtk-') + arch
        release = dict(schema=2, application_id='test.day.selfupdate', version='0.2.0', build=2,
                       archive=payload.name, size=payload.stat().st_size, sha256=hashlib.sha256(payload.read_bytes()).hexdigest(),
                       target=target, tag='v0.2.0', helper=dict(name=tool.name, size=tool.stat().st_size, sha256=hashlib.sha256(tool.read_bytes()).hexdigest()))
        metadata = inbox / 'release.json'; metadata.write_text(json.dumps(release))
        signature = run(tool, 'sign', metadata, key, capture_output=True, text=True).stdout
        (inbox / 'release.sig').write_text(signature)
        # A short-lived caller waits for readiness, then exits. The real helper must finish later.
        caller_code = 'import pathlib,time,sys; p=pathlib.Path(sys.argv[1]); end=time.monotonic()+25\nwhile not p.exists() and time.monotonic()<end: time.sleep(.05)'
        environment = {**os.environ, 'DYSU_FIXTURE_MARKER': str(marker), 'DYSU_FIXTURE_READY': str(inbox / 'ready')}
        caller = subprocess.Popen([str(executable), '--caller'] if WINDOWS else [sys.executable, '-c', caller_code, str(inbox / 'ready')], env=environment)
        request = dict(application_id=release['application_id'], current_build=1, key=public, target=target,
                       destination=str(destination), executable=str(executable), inbox=str(inbox), caller_pid=caller.pid)
        request_path = inbox / 'request.json'; request_path.write_text(json.dumps(request))
        environment = {**os.environ, 'DYSU_FIXTURE_MARKER': str(marker)}
        try:
            (inbox / 'release.sig').write_text('00' * 64)
            bad = subprocess.run([str(tool), 'desktop-install', str(request_path)], env=environment, capture_output=True)
            assert bad.returncode != 0 and destination.exists() and not marker.exists()
            (inbox / 'release.sig').write_text(signature)
            helper = subprocess.Popen([str(tool), 'desktop-install', str(request_path)], env=environment)
            assert helper.wait(timeout=35) == 0
            caller.wait(timeout=5)
            wait(marker.exists)
            assert marker.read_text() == 'new'
            session = base / ('.day-selfupdate-' + release['application_id'])
            assert (session / 'previous').exists()
            assert json.loads((inbox / 'outcome.json').read_text())['state'] == 'installed'
            print(f'{target}: PASS rejection, caller exit, replacement, backup, relaunch')
        finally:
            if caller.poll() is None:
                caller.terminate(); caller.wait()


if __name__ == '__main__': main()
