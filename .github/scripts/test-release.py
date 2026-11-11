"""Check that a release archive, directory, or AppImage finds its own runtime."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def check_release(source):
    source = source.resolve()
    appimage = source.suffix == '.AppImage'
    with tempfile.TemporaryDirectory(prefix='mitos-release-') as directory:
        root = Path(directory)
        if appimage:
            executable = root / source.name
            shutil.copyfile(source, executable)
            # CI artifacts lose executable bits. Leave the original file alone.
            executable.chmod(0o755)
        else:
            if source.is_file():
                shutil.unpack_archive(source, root / 'extracted')
                packages = list((root / 'extracted').iterdir())
                if len(packages) != 1 or not packages[0].is_dir():
                    raise RuntimeError('Expected one package directory in the archive')
                package = packages[0]
            else:
                package = root / 'package'
                shutil.copytree(source, package)
            executable = package / ('ms.exe' if os.name == 'nt' else 'ms')
            if os.name != 'nt' and not os.access(executable, os.X_OK):
                raise RuntimeError('Package binary is not executable')

        env = os.environ.copy()
        for name in ('MITOS_RUNTIME', 'CARGO_MANIFEST_DIR'):
            env.pop(name, None)
        env.update(
            MITOS_CONFIG_DIR=str(root / 'config' / 'mitos'),
            XDG_CONFIG_HOME=str(root / 'config'),
            XDG_DATA_HOME=str(root / 'data'),
            XDG_STATE_HOME=str(root / 'state'),
            APPDATA=str(root / 'config'),
            LOCALAPPDATA=str(root / 'data'),
            NO_COLOR='1',
        )
        if appimage:
            env['APPIMAGE_EXTRACT_AND_RUN'] = '1'
        subprocess.run([str(executable), '--version'], cwd=root, env=env, check=True)
        result = subprocess.run([str(executable), '--health', 'rust'], cwd=root, env=env,
                                stdout=subprocess.PIPE, text=True, encoding='utf-8')
        encoding = sys.stdout.encoding or 'utf-8'
        print(result.stdout.encode(encoding, errors='backslashreplace').decode(encoding), end='')
        result.check_returncode()
        for label in ('Tree-sitter parser:', 'Highlight queries:'):
            if not any(label in line and '✓' in line for line in result.stdout.splitlines()):
                raise RuntimeError(f'Bundled runtime check failed: {label}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    args = parser.parse_args()
    check_release(args.source)


if __name__ == '__main__':
    main()
