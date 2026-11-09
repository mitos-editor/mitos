"""Check that a release binary finds its bundled runtime outside the checkout."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

source = Path(sys.argv[1]).resolve()
binary = 'ms.exe' if os.name == 'nt' else 'ms'
with tempfile.TemporaryDirectory(prefix='mitos-release-') as directory:
    root = Path(directory)
    if source.is_file():
        shutil.unpack_archive(source, root / 'extracted')
        packages = list((root / 'extracted').iterdir())
        if len(packages) != 1 or not packages[0].is_dir():
            raise RuntimeError('Expected one package directory in the archive')
        package = packages[0]
    else:
        package = root / 'package'
        shutil.copytree(source, package)
    executable = package / binary
    if os.name != 'nt' and not os.access(executable, os.X_OK):
        raise RuntimeError('Package binary is not executable')
    env = {k: v for k, v in os.environ.items() if k not in ('MITOS_RUNTIME', 'CARGO_MANIFEST_DIR')}
    env.update(MITOS_CONFIG_DIR=str(root / 'config' / 'mitos'), XDG_CONFIG_HOME=str(root / 'config'), XDG_DATA_HOME=str(root / 'data'),
               XDG_STATE_HOME=str(root / 'state'), APPDATA=str(root / 'config'),
               LOCALAPPDATA=str(root / 'data'), NO_COLOR='1')
    subprocess.run([str(executable), '--version'], cwd=root, env=env, check=True)
    result = subprocess.run([str(executable), '--health', 'rust'], cwd=root, env=env,
                            check=True, capture_output=True, text=True, encoding='utf-8')
    print(result.stdout)
    for label in ('Tree-sitter parser:', 'Highlight queries:'):
        if not any(label in line and '✓' in line for line in result.stdout.splitlines()):
            raise RuntimeError(f'Bundled runtime check failed: {label}')
