"""Installer regression tests using local release archives and download fixtures."""
import hashlib
import io
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[2]
SHELL_INSTALLER = ROOT / 'book/install/install.sh'
PS_INSTALLER = ROOT / 'book/install/install.ps1'


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='mitos installers ')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.downloads = self.root / 'downloads'
        self.downloads.mkdir()
        self.tools = self.root / 'tools'
        self.tools.mkdir()
        self.install = self.root / 'installed versions'
        self.bin = self.root / 'bin directory'
        self.env = dict(os.environ, FIXTURE_DIR=str(self.downloads),
                        FIXTURE_OS='Darwin', FIXTURE_ARCH='arm64', FIXTURE_TAG='v0.1.0')
        self.env['PATH'] = str(self.tools) + os.pathsep + self.env['PATH']

    def archive(self, tag='v0.1.0', arch='aarch64', system='macos', bad_hash=False,
                missing_runtime=False):
        name = f'mitos-{tag}-{arch}-{system}'
        extension = 'zip' if system == 'windows' else 'tar.xz'
        path = self.downloads / f'{name}.{extension}'
        binary = 'ms.exe' if system == 'windows' else 'ms'
        content = f'#!/bin/sh\nprintf "Mitos {tag}\\n"\n'.encode()
        if extension == 'zip':
            with zipfile.ZipFile(path, 'w') as archive:
                archive.writestr(f'{name}/{binary}', content)
                if not missing_runtime:
                    archive.writestr(f'{name}/runtime/queries/test.scm', 'fixture')
        else:
            with tarfile.open(path, 'w:xz') as archive:
                info = tarfile.TarInfo(f'{name}/{binary}')
                info.mode = 0o755
                info.size = len(content)
                archive.addfile(info, io.BytesIO(content))
                if not missing_runtime:
                    info = tarfile.TarInfo(f'{name}/runtime/queries/test.scm')
                    info.size = 7
                    archive.addfile(info, io.BytesIO(b'fixture'))
        digest = '0' * 64 if bad_hash else hashlib.sha256(path.read_bytes()).hexdigest()
        manifest = self.downloads / f'{tag}.txt'
        with manifest.open('a') as output:
            output.write(f'{digest}  {path.name}\n')
        return path

    def shell(self, *args):
        if os.name == 'nt':
            self.skipTest('POSIX installer is exercised on Linux and macOS')
        (self.tools / 'uname').write_text(
            '#!/bin/sh\ncase "$1" in -s) echo "$FIXTURE_OS";; -m) echo "$FIXTURE_ARCH";; esac\n')
        (self.tools / 'curl').write_text('''#!/usr/bin/env python3
import os, pathlib, shutil, sys
args = sys.argv[1:]
url = next(arg for arg in args if arg.startswith('https://'))
if url.endswith('/latest'):
    print('https://github.com/mitos-editor/mitos/releases/tag/' + os.environ['FIXTURE_TAG'], end='')
    sys.exit(0)
name = url.rsplit('/', 1)[1]
if name == 'SHA256SUMS': name = url.split('/')[-2] + '.txt'
source = pathlib.Path(os.environ['FIXTURE_DIR']) / name
if not source.exists(): sys.exit(22)
out = args[args.index('--output') + 1]
shutil.copyfile(source, out)
''')
        for tool in self.tools.iterdir():
            tool.chmod(0o755)
        return subprocess.run(['sh', str(SHELL_INSTALLER), '--install-dir', str(self.install),
                               '--bin-dir', str(self.bin), *args], env=self.env,
                              capture_output=True, text=True)

    def powershell(self, *args):
        pwsh = os.environ.get('PWSH') or shutil.which('pwsh')
        if not pwsh:
            self.skipTest('PowerShell runtime is not installed')
        wrapper = self.root / 'test.ps1'
        wrapper.write_text('''$ErrorActionPreference = 'Stop'
$env:OS = 'Windows_NT'
$env:PROCESSOR_ARCHITECTURE = $env:FIXTURE_ARCH
$env:PROCESSOR_ARCHITEW6432 = ''
function Invoke-RestMethod { param($Uri) @{ tag_name = $env:FIXTURE_TAG } }
function Invoke-WebRequest {
    param($Uri, $OutFile, [switch]$UseBasicParsing)
    $name = ($Uri -split '/')[-1]
    if ($name -eq 'SHA256SUMS') { $name = ($Uri -split '/')[-2] + '.txt' }
    Copy-Item -LiteralPath (Join-Path $env:FIXTURE_DIR $name) -Destination $OutFile
}
& $env:INSTALLER -InstallDir $env:INSTALL_DIR -NoModifyPath @args
''')
        env = dict(self.env, INSTALLER=str(PS_INSTALLER), INSTALL_DIR=str(self.install))
        return subprocess.run([pwsh, '-NoProfile', '-NonInteractive', '-File', str(wrapper), *args],
                              env=env, capture_output=True, text=True)

    def test_shell_platforms_and_paths_with_spaces(self):
        for system, uname, arch, cpu in [('linux', 'Linux', 'x86_64', 'x86_64'),
                                        ('linux', 'Linux', 'aarch64', 'aarch64'),
                                        ('macos', 'Darwin', 'x86_64', 'x86_64'),
                                        ('macos', 'Darwin', 'aarch64', 'arm64')]:
            with self.subTest(system=system, arch=arch):
                self.archive(arch=arch, system=system)
                self.env.update(FIXTURE_OS=uname, FIXTURE_ARCH=cpu)
                result = self.shell('--version', '0.1.0')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue((self.bin/'ms').is_symlink())
                self.assertTrue((self.bin/'ms').resolve().parent.joinpath('runtime/queries/test.scm').exists())

    def test_shell_latest_reinstall_and_switch_versions(self):
        self.archive()
        self.assertEqual(self.shell().returncode, 0)
        self.assertEqual(self.shell().returncode, 0)
        self.archive(tag='v0.2.0')
        result = self.shell('--version', 'v0.2.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('v0.2.0', str((self.bin/'ms').resolve()))
        self.assertTrue((self.install/'v0.1.0-aarch64-macos/runtime').exists())
        self.assertEqual(self.shell('--version', '0.1.0').returncode, 0)
        self.assertIn('v0.1.0', str((self.bin/'ms').resolve()))

    def test_shell_bad_checksum_does_not_change_existing_install(self):
        self.archive()
        self.assertEqual(self.shell().returncode, 0)
        before = (self.bin/'ms').readlink()
        self.archive(tag='v0.2.0', bad_hash=True)
        result = self.shell('--version', '0.2.0')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Checksum mismatch', result.stderr)
        self.assertEqual((self.bin/'ms').readlink(), before)
        self.assertFalse((self.install/'v0.2.0-aarch64-macos').exists())

    def test_shell_preserves_other_executable(self):
        self.archive()
        self.bin.mkdir()
        (self.bin/'ms').write_text('another installation')
        result = self.shell()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.bin/'ms').read_text(), 'another installation')

    def test_shell_rejects_incomplete_archive(self):
        self.archive(missing_runtime=True)
        result = self.shell()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.install.exists())

    def test_shell_rejects_unsupported_platform_and_invalid_arguments(self):
        self.env['FIXTURE_ARCH'] = 'riscv64'
        self.assertNotEqual(self.shell().returncode, 0)
        self.env['FIXTURE_ARCH'] = 'arm64'
        for args in [('--version', '../bad'), ('--version',), ('--unknown',)]:
            with self.subTest(args=args):
                self.assertNotEqual(self.shell(*args).returncode, 0)

    def test_powershell_architectures_and_version_switch(self):
        for arch, cpu in [('aarch64', 'ARM64'), ('x86_64', 'AMD64')]:
            with self.subTest(arch=arch):
                self.archive(arch=arch, system='windows')
                self.env['FIXTURE_ARCH'] = cpu
                result = self.powershell()
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                launcher = self.install/'bin/ms.cmd'
                self.assertIn(f'v0.1.0-{arch}-windows', launcher.read_text())
                self.assertTrue((self.install/f'versions/v0.1.0-{arch}-windows/runtime').exists())
        self.archive(tag='v0.2.0', arch='x86_64', system='windows')
        result = self.powershell('-Version', '0.2.0')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('v0.2.0', (self.install/'bin/ms.cmd').read_text())

    def test_powershell_bad_checksum_does_not_install(self):
        self.archive(arch='x86_64', system='windows', bad_hash=True)
        self.env['FIXTURE_ARCH'] = 'AMD64'
        result = self.powershell()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Checksum mismatch', result.stderr)
        self.assertFalse(self.install.exists())

    def test_powershell_preserves_other_launcher(self):
        self.archive(arch='x86_64', system='windows')
        self.env['FIXTURE_ARCH'] = 'AMD64'
        launcher = self.install/'bin/ms.cmd'
        launcher.parent.mkdir(parents=True)
        launcher.write_text('another installation')
        result = self.powershell()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(launcher.read_text(), 'another installation')


if __name__ == '__main__':
    unittest.main()
