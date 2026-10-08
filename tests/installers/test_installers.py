"""Installer regression tests using local release archives and download fixtures."""
import hashlib
import io
import os
import re
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
FIXTURES = Path(__file__).parent / 'fixtures'


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='mitos installers ')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.downloads = self.root / 'downloads'
        self.downloads.mkdir()
        self.tools = self.root / 'tools'
        self.tools.mkdir()
        for source, name in [('uname.sh', 'uname'), ('curl.py', 'curl')]:
            tool = self.tools / name
            shutil.copyfile(FIXTURES / source, tool)
            tool.chmod(0o755)
        self.ps_wrapper = self.root / 'test.ps1'
        shutil.copyfile(FIXTURES / 'powershell.ps1', self.ps_wrapper)
        self.download_log = self.root / 'downloads.log'
        self.download_log.touch()
        self.install = self.root / 'installed versions'
        self.bin = self.root / 'bin directory'
        self.env = dict(os.environ, FIXTURE_DIR=str(self.downloads),
                        FIXTURE_DOWNLOAD_LOG=str(self.download_log),
                        FIXTURE_OS='Darwin', FIXTURE_ARCH='arm64', FIXTURE_TAG='v0.1.0')
        self.env['PATH'] = str(self.tools) + os.pathsep + self.env['PATH']

    def create_archive(self, tag='v0.1.0', arch='aarch64', system='macos', bad_hash=False,
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

    def run_shell(self, *args, script=None, use_default_install_dir=False):
        if os.name == 'nt':
            self.skipTest('POSIX installer is exercised on Linux and macOS')
        command = ['sh', str(SHELL_INSTALLER)] if script is None else ['sh', '-s', '--']
        if not use_default_install_dir:
            command += ['--install-dir', str(self.install)]
        command += ['--bin-dir', str(self.bin), *args]
        return subprocess.run(command, env=self.env, input=script,
                              capture_output=True, text=True)

    def run_powershell(self, *args):
        pwsh = os.environ.get('PWSH') or shutil.which('pwsh')
        if not pwsh:
            self.skipTest('PowerShell runtime is not installed')
        env = dict(self.env, INSTALLER=str(PS_INSTALLER), INSTALL_DIR=str(self.install))
        return subprocess.run([pwsh, '-NoProfile', '-NonInteractive', '-File', str(self.ps_wrapper), *args],
                              env=env, capture_output=True, text=True)

    def assert_shell_platform(self, system, uname, arch, cpu):
        self.create_archive(arch=arch, system=system)
        self.env.update(FIXTURE_OS=uname, FIXTURE_ARCH=cpu)
        result = self.run_shell('--version', '0.1.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        executable = self.bin / 'ms'
        expected = self.install / f'v0.1.0-{arch}-{system}' / 'ms'
        self.assertTrue(executable.is_symlink())
        self.assertEqual(executable.resolve(), expected.resolve())
        self.assertTrue((expected.parent / 'runtime/queries/test.scm').exists())

    def test_shell_linux_x86_64(self):
        self.assert_shell_platform('linux', 'Linux', 'x86_64', 'x86_64')

    def test_shell_linux_aarch64(self):
        self.assert_shell_platform('linux', 'Linux', 'aarch64', 'aarch64')

    def test_shell_macos_x86_64(self):
        self.assert_shell_platform('macos', 'Darwin', 'x86_64', 'x86_64')

    def test_shell_macos_aarch64(self):
        self.assert_shell_platform('macos', 'Darwin', 'aarch64', 'arm64')

    def test_shell_latest_reinstall_and_switch_versions(self):
        self.create_archive()
        self.assertEqual(self.run_shell().returncode, 0)
        self.assertEqual(self.run_shell().returncode, 0)
        self.create_archive(tag='v0.2.0')
        result = self.run_shell('--version', 'v0.2.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('v0.2.0', str((self.bin/'ms').resolve()))
        self.assertTrue((self.install/'v0.1.0-aarch64-macos/runtime').exists())
        self.assertEqual(self.run_shell('--version', '0.1.0').returncode, 0)
        self.assertIn('v0.1.0', str((self.bin/'ms').resolve()))

    def assert_no_shell_install_started(self):
        self.assertEqual(self.download_log.read_text(), '')
        self.assertFalse(self.install.exists())
        self.assertFalse(self.bin.exists())

    def test_shell_without_entrypoint_does_not_install(self):
        source = SHELL_INSTALLER.read_text()
        entrypoint = re.search(r'(?m)^\{\s*\n\s*main "\$@"\s*\n\}\s*\Z', source)
        self.assertIsNotNone(entrypoint, 'Expected a final block invoking main')
        result = self.run_shell(script=source[:entrypoint.start()])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_no_shell_install_started()

    def test_shell_incomplete_entrypoint_fails_without_installing(self):
        source = SHELL_INSTALLER.read_text().rstrip()
        self.assertTrue(source.endswith('}'), 'Expected a closing entrypoint brace')
        result = self.run_shell(script=source[:-1])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('syntax error', result.stderr.lower())
        self.assert_no_shell_install_started()

    def test_shell_respects_xdg_data_home(self):
        self.create_archive()
        data_home = self.root / 'custom data directory'
        self.env['XDG_DATA_HOME'] = str(data_home)
        result = self.run_shell(use_default_install_dir=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((data_home/'mitos/v0.1.0-aarch64-macos/runtime').exists())
        self.assertEqual((self.bin/'ms').resolve().parent,
                         (data_home/'mitos/v0.1.0-aarch64-macos').resolve())

    def test_shell_bad_checksum_does_not_change_existing_install(self):
        self.create_archive()
        self.assertEqual(self.run_shell().returncode, 0)
        before = (self.bin/'ms').readlink()
        self.create_archive(tag='v0.2.0', bad_hash=True)
        result = self.run_shell('--version', '0.2.0')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Checksum mismatch', result.stderr)
        self.assertEqual((self.bin/'ms').readlink(), before)
        self.assertFalse((self.install/'v0.2.0-aarch64-macos').exists())

    def test_shell_preserves_other_executable(self):
        self.create_archive()
        self.bin.mkdir()
        (self.bin/'ms').write_text('another installation')
        result = self.run_shell()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.bin/'ms').read_text(), 'another installation')

    def test_shell_rejects_incomplete_archive(self):
        self.create_archive(missing_runtime=True)
        result = self.run_shell()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.install.exists())

    def test_shell_rejects_unsupported_platform_and_invalid_arguments(self):
        self.env['FIXTURE_ARCH'] = 'riscv64'
        self.assertNotEqual(self.run_shell().returncode, 0)
        self.env['FIXTURE_ARCH'] = 'arm64'
        for args in [('--version', '../bad'), ('--version',), ('--unknown',)]:
            with self.subTest(args=args):
                self.assertNotEqual(self.run_shell(*args).returncode, 0)

    def assert_powershell_platform(self, arch, cpu):
        self.create_archive(arch=arch, system='windows')
        self.env['FIXTURE_ARCH'] = cpu
        result = self.run_powershell()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        launcher = self.install / 'bin/ms.cmd'
        expected = self.install / 'versions' / f'v0.1.0-{arch}-windows'
        self.assertIn(f'..\\versions\\{expected.name}\\ms.exe', launcher.read_text())
        self.assertTrue((expected / 'ms.exe').is_file())
        self.assertTrue((expected / 'runtime/queries/test.scm').is_file())

    def test_powershell_x86_64(self):
        self.assert_powershell_platform('x86_64', 'AMD64')

    def test_powershell_aarch64(self):
        self.assert_powershell_platform('aarch64', 'ARM64')

    def test_powershell_switch_versions(self):
        self.create_archive(arch='x86_64', system='windows')
        self.env['FIXTURE_ARCH'] = 'AMD64'
        result = self.run_powershell()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.create_archive(tag='v0.2.0', arch='x86_64', system='windows')
        result = self.run_powershell('-Version', '0.2.0')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('..\\versions\\v0.2.0-x86_64-windows\\ms.exe',
                      (self.install / 'bin/ms.cmd').read_text())
        self.assertTrue((self.install / 'versions/v0.1.0-x86_64-windows/runtime').exists())

    def test_powershell_bad_checksum_does_not_install(self):
        self.create_archive(arch='x86_64', system='windows', bad_hash=True)
        self.env['FIXTURE_ARCH'] = 'AMD64'
        result = self.run_powershell()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Checksum mismatch', result.stderr)
        self.assertFalse(self.install.exists())

    def test_powershell_one_liner_rejects_old_powershell(self):
        self.env.update(FIXTURE_ARCH='AMD64', FIXTURE_PS_VERSION='5.0')
        result = self.run_powershell()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('PowerShell 5.1 or newer is required', result.stderr)
        self.assertFalse(self.install.exists())

    def test_powershell_preserves_other_launcher(self):
        self.create_archive(arch='x86_64', system='windows')
        self.env['FIXTURE_ARCH'] = 'AMD64'
        launcher = self.install/'bin/ms.cmd'
        launcher.parent.mkdir(parents=True)
        launcher.write_text('another installation')
        result = self.run_powershell()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(launcher.read_text(), 'another installation')


if __name__ == '__main__':
    unittest.main()
