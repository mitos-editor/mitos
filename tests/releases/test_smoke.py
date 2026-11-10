"""Exercise the release smoke checker with local executables."""
import contextlib
import importlib.util
import io
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('smoke', ROOT / '.github/scripts/test-release.py')
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)

BINARY = '''#!/usr/bin/env python3
import os
from pathlib import Path
import sys

assert 'MITOS_RUNTIME' not in os.environ
assert 'CARGO_MANIFEST_DIR' not in os.environ
assert Path(os.environ['MITOS_CONFIG_DIR']).parent.resolve() == Path.cwd() / 'config'
if Path(__file__).suffix == '.AppImage':
    assert os.environ['APPIMAGE_EXTRACT_AND_RUN'] == '1'
if sys.argv[1] == '--version':
    sys.exit(0)
elif os.environ.get('FIXTURE_HEALTH_ERROR'):
    print('health failure details')
    sys.exit(17)
else:
    print('Tree-sitter parser: ✓')
    print('Highlight queries: ✓')
'''


@unittest.skipIf(os.name == 'nt', 'Fixtures use POSIX executable scripts')
class SmokeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.package = self.root / 'mitos-fixture'
        self.package.mkdir()
        self.binary = self.package / 'ms'
        self.binary.write_text(BINARY, encoding='utf-8')
        self.binary.chmod(0o755)

    def check(self, source):
        with patch.dict(os.environ, MITOS_RUNTIME='other-runtime',
                        MITOS_CONFIG_DIR='other-config', CARGO_MANIFEST_DIR='other-workspace'), \
             contextlib.redirect_stdout(io.StringIO()) as output:
            smoke.check_release(source)
        return output.getvalue()

    def test_directory_and_archive_run_away_from_user_runtime(self):
        self.assertIn('Highlight queries: ✓', self.check(self.package))
        archive = Path(shutil.make_archive(str(self.root / 'release'), 'xztar',
                                          root_dir=self.root, base_dir=self.package.name))
        self.assertIn('Highlight queries: ✓', self.check(archive))

    def test_appimage_uses_extraction_without_changing_input_permissions(self):
        image = self.root / 'mitos.AppImage'
        image.write_text(BINARY, encoding='utf-8')
        image.chmod(0o644)
        self.assertIn('Tree-sitter parser: ✓', self.check(image))
        self.assertEqual(image.stat().st_mode & 0o777, 0o644)

    def test_health_failure_keeps_diagnostics(self):
        with patch.dict(os.environ, FIXTURE_HEALTH_ERROR='1'), \
             contextlib.redirect_stdout(io.StringIO()) as output:
            with self.assertRaises(subprocess.CalledProcessError) as error:
                smoke.check_release(self.package)
        self.assertEqual(error.exception.returncode, 17)
        self.assertIn('health failure details', output.getvalue())


if __name__ == '__main__':
    unittest.main()
