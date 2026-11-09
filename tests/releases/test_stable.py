"""Exercise stable updates against local Git repositories."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class StableTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.remote = self.root / 'origin.git'
        self.work = self.root / 'work'
        self.work.mkdir()
        self.env = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM='1',
                        GITHUB_REPOSITORY='example/mitos', LATEST='v0.2.0')
        self.git('init', '--bare', str(self.remote))
        self.git('init', '-b', 'main')
        self.git('config', 'user.name', 'Release test')
        self.git('config', 'user.email', 'test@example.invalid')
        self.git('remote', 'add', 'origin', str(self.remote))
        self.git('config', f'url.{self.remote}.insteadOf', 'https://github.com/example/mitos.git')
        self.git('commit', '--allow-empty', '-m', 'first')
        self.first = self.git('rev-parse', 'HEAD')
        self.git('tag', '-a', 'v0.1.0', '-m', 'first release')
        self.git('commit', '--allow-empty', '-m', 'second')
        self.second = self.git('rev-parse', 'HEAD')
        self.git('tag', '-a', 'v0.2.0', '-m', 'second release')
        self.git('push', 'origin', 'main', '--tags')
        tools = self.root / 'tools'
        tools.mkdir()
        gh = tools / 'gh'
        gh.write_text('#!/bin/sh\nprintf "%s\\n" "$LATEST"\n')
        gh.chmod(0o755)
        self.env['PATH'] = str(tools) + os.pathsep + self.env['PATH']

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.work, env=self.env,
                                       stderr=subprocess.DEVNULL, text=True).strip()

    def update(self):
        return subprocess.run(['bash', str(ROOT / '.github/scripts/update-stable.sh')],
                              cwd=self.work, env=self.env, capture_output=True, text=True)

    def stable(self):
        return self.git('--git-dir', str(self.remote), 'rev-parse', 'refs/heads/stable')

    def test_bootstrap_and_annotated_tag_update(self):
        self.env['LATEST'] = 'v0.1.0'
        result = self.update()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.stable(), self.first)
        self.env['LATEST'] = 'v0.2.0'
        result = self.update()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.stable(), self.second)
        self.assertEqual(self.update().returncode, 0)

    def test_older_release_does_not_move_stable_backwards(self):
        self.assertEqual(self.update().returncode, 0)
        self.env['LATEST'] = 'v0.1.0'
        result = self.update()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.stable(), self.second)

    def test_divergent_hotfix_requires_manual_resolution(self):
        self.assertEqual(self.update().returncode, 0)
        self.git('checkout', '-b', 'maintenance', self.first)
        self.git('commit', '--allow-empty', '-m', 'hotfix')
        self.git('tag', 'v0.1.1')
        self.git('push', 'origin', 'v0.1.1')
        self.env['LATEST'] = 'v0.1.1'
        self.assertNotEqual(self.update().returncode, 0)
        self.assertEqual(self.stable(), self.second)

    def test_latest_changes_during_fetch(self):
        self.assertEqual(self.update().returncode, 0)
        self.env['COUNTER'] = str(self.root / 'calls')
        (self.root / 'tools/gh').write_text(
            '#!/bin/sh\n'
            'if [ -e "$COUNTER" ]; then echo v0.1.0; else touch "$COUNTER"; echo v0.2.0; fi\n')
        result = self.update()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Latest changed', result.stderr)
        self.assertEqual(self.stable(), self.second)

    def test_invalid_latest_is_rejected(self):
        self.env['LATEST'] = 'nightly-2026-11-09-abcdef123456'
        result = self.update()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('not a stable version', result.stderr)


if __name__ == '__main__':
    unittest.main()
