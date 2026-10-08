#!/usr/bin/env python3
"""Secret-free build configuration and updater payload staging for dayapp.yml."""
import argparse
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
DEMO = ROOT / 'demo'


def run(*args):
    subprocess.run([str(a) for a in args], check=True, cwd=ROOT)


def settings(target):
    app = tomllib.loads((DEMO / 'Day.toml').read_text())['app']
    package = tomllib.loads((DEMO / 'Cargo.toml').read_text())['package']
    version = package['version']
    tag = os.environ.get('GITHUB_REF_NAME', '')
    release = os.environ.get('GITHUB_REF_TYPE') == 'tag'
    if release and tag != 'v' + version:
        raise ValueError('Tag must equal v + demo/Cargo.toml version; bump demo version and Day.toml build before tagging')
    key = os.environ.get('DAY_UPDATE_PUBLIC_KEY', '').strip().lower()
    if key and not re.fullmatch('[0-9a-fA-F]{64}', key):
        raise ValueError('DAY_UPDATE_PUBLIC_KEY must be a hex Ed25519 public key')
    if release and not key:
        raise ValueError('Set the DAY_UPDATE_PUBLIC_KEY repository variable before releasing')
    arch = {'arm64': 'aarch64', 'AMD64': 'x86_64'}.get(platform.machine(), platform.machine())
    return app, version, key, f'{target}-{arch}', release


def configure(target):
    app, version, key, triple, release = settings(target)
    team = os.environ.get('DAY_UPDATE_APPLE_TEAM', '')
    if release and target.startswith('macos-') and not re.fullmatch('[A-Z0-9]{10}', team):
        raise ValueError('Set DAY_UPDATE_APPLE_TEAM to the Developer ID signing Team ID')
    values = dict(DAY_UPDATE_BUILD=str(app['build']), DAY_UPDATE_VERSION=version,
                  DAY_UPDATE_PUBLIC_KEY=key, DAY_UPDATE_TARGET=triple,
                  DAY_UPDATE_REPOSITORY=os.environ.get('GITHUB_REPOSITORY', 'daybrite/day-piece-selfupdate'))
    path = os.environ.get('GITHUB_ENV')
    if path:
        with open(path, 'a') as stream:
            for name, value in values.items():
                if '\n' in value or '\r' in value:
                    raise ValueError('invalid configuration value')
                stream.write(f'{name}={value}\n')
    else:
        print(json.dumps(values, indent=2))


def plist(path, data=None):
    if data is None:
        return plistlib.loads(path.read_bytes())
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(plistlib.dumps(data))


def stage(target):
    app, version, key, triple, release = settings(target)
    dist = DEMO / 'build/day/dist'
    run('cargo', 'build', '--release', '-p', 'day-selfupdate-tools')
    helper = ROOT / 'target/release' / ('day-selfupdate-tool.exe' if target.startswith('windows-') else 'day-selfupdate-tool')
    metadata = dict(schema=2, application_id=app['id'], version=version, build=app['build'], target=triple)
    if target == 'macos-appkit':
        bundles = list((DEMO / 'build/day/pack/macos-appkit').glob('*.app'))
        if len(bundles) != 1:
            raise ValueError('expected one packed macOS app')
        bundle = bundles[0]
        service_id = app['id'] + '.installer'
        service = bundle / 'Contents/XPCServices' / (service_id + '.xpc')
        (service / 'Contents/MacOS').mkdir(parents=True, exist_ok=True)
        helpers = bundle / 'Contents/Helpers'
        helpers.mkdir(exist_ok=True)
        shutil.copy2(helper, helpers / helper.name)
        run('xcrun', 'clang', '-fobjc-arc', '-fblocks', '-mmacosx-version-min=13.0', '-framework', 'Foundation',
            ROOT / 'platform/macos/service.m', '-o', service / 'Contents/MacOS/Launcher')
        team = os.environ.get('DAY_UPDATE_APPLE_TEAM', '')
        anchor = f' and anchor apple generic and certificate leaf[subject.OU] = "{team}"' if release else ''
        plist(service / 'Contents/Info.plist', dict(CFBundleIdentifier=service_id, CFBundleExecutable='Launcher',
              CFBundlePackageType='XPC!', CFBundleVersion=str(app['build']), XPCService={'ServiceType': 'Application'},
              DYSUClientRequirement=f'identifier "{app["id"]}"{anchor}'))
        info_path = bundle / 'Contents/Info.plist'
        info = plist(info_path)
        info.update(DYSUPublicKey=key, DYSUDevelopment=not release, DYSUTarget=triple,
                    DYSURepository=os.environ.get('GITHUB_REPOSITORY', 'daybrite/day-piece-selfupdate'),
                    DYSUServiceIdentifier=service_id, DYSUServiceRequirement=f'identifier "{service_id}"{anchor}')
        plist(info_path, info)
        run('/usr/bin/codesign', '--force', '--sign', '-', helpers / helper.name)
        run('/usr/bin/codesign', '--force', '--sign', '-', service)
        run('/usr/bin/codesign', '--force', '--sign', '-', '--entitlements', DEMO / 'platform/macos/Updater.entitlements', bundle)
        # The isolated sign-macos job replaces this ZIP with its signed and stapled counterpart.
        archive = 'selfupdate-demo-macos-appkit-update.zip'
        run('/usr/bin/ditto', '--norsrc', '--noextattr', '--noqtn', '-c', '-k', '--keepParent', bundle, dist / archive)
        # Recreate the development DMG too, so non-tag artifacts contain the same sandboxed app.
        dmgs = list(dist.glob('*.dmg'))
        if len(dmgs) != 1:
            raise ValueError('expected one packed DMG')
        staging = DEMO / 'build/day/update-dmg'
        if staging.exists(): shutil.rmtree(staging)
        staging.mkdir()
        run('/usr/bin/ditto', bundle, staging / bundle.name)
        (staging / 'Applications').symlink_to('/Applications')
        run('/usr/bin/hdiutil', 'create', '-quiet', '-srcfolder', staging, '-ov', '-format', 'UDZO', dmgs[0])
        metadata.update(archive=archive, helper=None)
    else:
        suffix = '*-setup.exe' if target.startswith('windows-') else '*.appimage'
        artifacts = list(dist.glob(suffix))
        if len(artifacts) != 1: raise ValueError(f'expected one {suffix}')
        name = 'day-selfupdate-helper-' + triple + ('.exe' if target.startswith('windows-') else '')
        shutil.copy2(helper, dist / name)
        metadata.update(archive=artifacts[0].name, helper={'name': name})
    (dist / f'update-{triple}.unsigned.json').write_text(json.dumps(metadata, indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['configure', 'stage'])
    parser.add_argument('target', choices=['macos-appkit', 'windows-winui', 'linux-gtk'])
    args = parser.parse_args()
    (configure if args.action == 'configure' else stage)(args.target)
