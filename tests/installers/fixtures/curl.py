#!/usr/bin/env python3
"""Serve local release fixtures and record every installer download request."""
import os
from pathlib import Path
import shutil
import sys

args = sys.argv[1:]
with Path(os.environ['FIXTURE_DOWNLOAD_LOG']).open('a') as log:
    log.write(' '.join(args) + '\n')

assert args[args.index('--proto') + 1] == '=https'
assert args[args.index('--proto-redir') + 1] == '=https'
assert '--tlsv1.2' in args

url = next(arg for arg in args if arg.startswith('https://'))
if url.endswith('/latest'):
    tag = os.environ['FIXTURE_TAG']
    print(f'https://github.com/mitos-editor/mitos/releases/tag/{tag}', end='')
    sys.exit(0)

name = url.rsplit('/', 1)[1]
if name == 'SHA256SUMS':
    name = url.split('/')[-2] + '.txt'
source = Path(os.environ['FIXTURE_DIR']) / name
if not source.exists():
    sys.exit(22)
output = args[args.index('--output') + 1]
shutil.copyfile(source, output)
