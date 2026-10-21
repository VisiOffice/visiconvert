#!/usr/bin/env python3
"""Copy canonical SVG identity assets to application and marketing public roots."""
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[1]
source = ROOT / 'assets/brand'
for project in ['web', 'website']:
    if not (ROOT / project).is_dir():
        continue
    target = ROOT / project / 'public'
    (target / 'brand').mkdir(parents=True, exist_ok=True)
    for asset in source.glob('*.svg'):
        if asset.name == 'favicon.svg':
            shutil.copyfile(asset, target / asset.name)
        else:
            shutil.copyfile(asset, target / 'brand' / asset.name)
    shutil.copyfile(ROOT / 'LICENSE', target / 'LICENSE.txt')
