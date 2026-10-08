#!/usr/bin/env python3
"""Generate ignored Cargo configuration pointing canonical Day dependencies at an adjacent tree."""
from pathlib import Path
import tomllib
root = Path(__file__).resolve().parents[1]
day = root.parent / 'day'
packages = {tomllib.loads(p.read_text()).get('package', {}).get('name'): p.parent
            for p in (day / 'crates').glob('*/Cargo.toml')}
for directory in [root, root / 'demo']:
    names = ['day-core', 'day-pieces', 'day-reactive']
    if directory != root: names += ['day', 'day-build']
    patches = [f'{name} = {{ path = "{packages[name]}" }}' for name in names]
    config = directory / '.cargo/config.toml'
    config.parent.mkdir(exist_ok=True)
    config.write_text('[patch."https://github.com/daybrite/day"]\n' + '\n'.join(patches) + '\n')
print('Configured local Day patches for both workspaces (ignored by Git).')
