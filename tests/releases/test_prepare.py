"""Exercise release preparation with local Git/Cargo fixtures and mocked GitHub writes."""
from contextlib import chdir
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('release', ROOT / '.github/scripts/release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
REPO = 'example/mitos'


class ReleaseFixture(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.work = Path(temporary.name)
        environment = patch.dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull,
                                 GIT_CONFIG_NOSYSTEM='1', CARGO_NET_OFFLINE='true')
        environment.start()
        self.addCleanup(environment.stop)
        self.git('init', '-b', 'main')
        self.git('config', 'core.fsmonitor', 'false')
        self.git('config', 'user.name', 'Release test')
        self.git('config', 'user.email', 'release@example.invalid')
        self.manifest = self.work / 'Cargo.toml'
        self.manifest.write_text(
            '[workspace]\nmembers = ["term", "helper"]\nresolver = "3"\n\n'
            '[workspace.package]\nversion = "0.2.1"\nedition = "2024"\n')
        for name in ('term', 'helper'):
            self.package(name)
        shutil.copy(ROOT / 'cliff.toml', self.work)
        self.history = '## [0.2.1] - 2026-11-09\n\n- Previous curated release.\n'
        self.changelog = self.work / 'CHANGELOG.md'
        self.changelog.write_text('# Changelog\n\n## [Unreleased]\n\n' + self.history)
        self.commit('feat: initial release')
        self.git('tag', '-a', 'v0.2.1', '-m', 'release')

    def git(self, *args, **kwargs):
        return subprocess.check_output(['git', *args], cwd=self.work, text=True,
                                       stderr=subprocess.DEVNULL, **kwargs).strip()

    def commit(self, message):
        self.git('add', '.')
        self.git('commit', '-m', message)
        return self.git('rev-parse', 'HEAD')

    def package(self, name, version='version.workspace = true'):
        path = self.work / name
        (path / 'src').mkdir(parents=True)
        (path / 'src/lib.rs').write_text('pub fn example() {}\n')
        (path / 'Cargo.toml').write_text(
            f'[package]\nname = "{Path(name).name}"\n{version}\nedition.workspace = true\n')

    def merge_release(self):
        self.manifest.write_text(self.manifest.read_text().replace('0.2.1', '0.2.2'))
        self.changelog.write_text(
            '# Changelog\n\n## [Unreleased]\n\n'
            '## [0.2.2](https://example.invalid/compare) - 2026-11-15\n\n'
            '- Curated release notes.\n\n' + self.history)
        return self.commit('chore(release): prepare v0.2.2')


@unittest.skipUnless(shutil.which('git-cliff') and shutil.which('cargo'),
                     'Install git-cliff 2.14.2 and Cargo to run preparation fixtures')
class PrepareTests(ReleaseFixture):
    def prepare(self):
        with chdir(self.work):
            return release.prepare()

    def test_repository_changes_new_crate_and_handwritten_notes(self):
        vendor = 'vendor/lsp-types'
        self.package(vendor, 'version = "0.97.0"\npublish = false')
        vendor_manifest = (self.work / vendor / 'Cargo.toml').read_text()
        self.manifest.write_text(self.manifest.read_text().replace('"helper"]', f'"helper", "{vendor}"]'))
        (self.work / 'theme.toml').write_text('background = "black"\n')
        self.commit('feat(theme): add a theme (#42)')
        self.git('tag', 'nightly-2026-11-15-abcdef123456')
        self.git('tag', 'v0.2.2-beta.1')
        (self.work / 'guide.md').write_text('How to use the CLI\n')
        self.commit('docs: explain the CLI')
        self.changelog.write_text(self.changelog.read_text().replace(
            '## [Unreleased]\n', '## [Unreleased]\n\n- Handwritten upgrade note.\n'))
        self.commit('docs: record upgrade notes')
        self.assertEqual(self.prepare(), '0.2.2')
        self.assertEqual(release.workspace_version(self.manifest.read_text()), '0.2.2')
        self.assertEqual((self.work / vendor / 'Cargo.toml').read_text(), vendor_manifest)
        lock = tomllib.loads((self.work / 'Cargo.lock').read_text())
        self.assertEqual({p['name']: p['version'] for p in lock['package']},
                         {'term': '0.2.2', 'helper': '0.2.2', 'lsp-types': '0.97.0'})
        changelog = self.changelog.read_text()
        self.assertTrue(changelog.endswith(self.history))
        notes = release.release_notes(changelog, '0.2.2')
        for text in ('Handwritten upgrade note.', 'add a theme', 'explain the CLI',
                     '[#42](https://github.com/mitos-editor/mitos/pull/42)'):
            self.assertIn(text, notes)
        self.assertNotIn('Previous curated release.', notes)
        self.assertIn('compare/v0.2.1...v0.2.2', changelog)
        self.assertTrue(changelog.startswith('# Changelog\n\n## [Unreleased]\n\n## [0.2.2]'))

    def test_breaking_change_bumps_minor_below_one(self):
        self.git('commit', '--allow-empty', '-m', 'feat!: change commands')
        self.assertEqual(self.prepare(), '0.3.0')

    def test_no_commits_leaves_files_unchanged(self):
        self.assertIsNone(self.prepare())
        self.assertEqual(self.git('status', '--porcelain'), '')

    def test_unconventional_runtime_commit_is_included(self):
        (self.work / 'runtime.txt').write_text('a grammar revision\n')
        self.commit('Update grammar revision')
        self.assertEqual(self.prepare(), '0.2.2')
        self.assertIn('Update grammar revision', release.release_notes(self.changelog.read_text(), '0.2.2'))

    def test_unrelated_stable_tag_is_not_a_baseline(self):
        unrelated = self.git('commit-tree', 'HEAD^{tree}', input='unrelated upstream release\n')
        self.git('tag', 'v99.0.0', unrelated)
        self.git('tag', '25.07')
        self.git('commit', '--allow-empty', '-m', 'fix: new change')
        self.assertEqual(self.prepare(), '0.2.2')

    def test_next_release_does_not_repeat_previous_changes(self):
        self.git('commit', '--allow-empty', '-m', 'fix: first change')
        self.assertEqual(self.prepare(), '0.2.2')
        self.commit('chore(release): prepare v0.2.2')
        self.git('tag', 'v0.2.2')
        self.git('commit', '--allow-empty', '-m', 'fix: second change')
        self.assertEqual(self.prepare(), '0.2.3')
        notes = release.release_notes(self.changelog.read_text(), '0.2.3')
        self.assertIn('second change', notes)
        self.assertNotIn('first change', notes)

    def test_missing_unreleased_section_does_not_bump_version(self):
        original = self.manifest.read_text()
        self.changelog.write_text(self.history)
        self.commit('fix: something changed')
        with self.assertRaisesRegex(ValueError, 'Unreleased section'):
            self.prepare()
        self.assertEqual(self.manifest.read_text(), original)


class DraftTests(ReleaseFixture):
    def setUp(self):
        super().setUp()
        self.ref = self.merge_release()
        self.existing = []
        self.remote_tag = None
        self.fail_tag = False
        self.merged = []
        self.calls = []
        original_run = release.run

        def run(*args, **kwargs):
            if args[0] == 'gh':
                return json.dumps([self.existing])
            if args[0] == 'git' and 'push' in args:
                sha, ref = args[-1].split(':')
                self.calls.append(('PUSH', ref, {'sha': sha}))
                if self.fail_tag:
                    raise RuntimeError('Tag creation failed')
                self.remote_tag = sha
                return ''
            return original_run(*args, **kwargs)

        for name, effect in [('api', self.api), ('run', run)]:
            patcher = patch.object(release, name, side_effect=effect)
            patcher.start()
            self.addCleanup(patcher.stop)

    def api(self, repo, path, *, method='GET', **fields):
        self.assertEqual(repo, REPO)
        self.calls.append((method, path, fields))
        if path.startswith('commits/') and path.endswith('/pulls'):
            return self.merged
        if path.startswith('git/matching-refs/'):
            return [{'ref': 'refs/tags/v0.2.2'}] if self.remote_tag else []
        if path == 'commits/v0.2.2':
            return {'sha': self.remote_tag}
        if path == 'releases' and method == 'POST':
            self.existing.append(dict(fields, id=42))
            return self.existing[-1]
        self.fail(f'Unexpected API request: {method} {path}')

    def draft(self):
        with chdir(self.work):
            release.draft(REPO)

    def writes(self):
        return [call for call in self.calls if call[0] != 'GET']

    def test_draft_precedes_tag_and_uses_version_commit_notes(self):
        self.changelog.write_text(self.changelog.read_text().replace('Curated release notes.', 'Later edits.'))
        self.manifest.write_text(self.manifest.read_text() + '# An unrelated dependency change\n')
        self.commit('docs: edits after the release merge')
        self.draft()
        self.assertEqual([call[1] for call in self.writes()], ['releases', 'refs/tags/v0.2.2'])
        fields = self.writes()[0][2]
        self.assertEqual(fields['body'], '- Curated release notes.')
        self.assertEqual(fields['target_commitish'], self.ref)
        self.assertIs(fields['draft'], True)
        self.assertIs(fields['prerelease'], False)
        self.assertEqual(fields['make_latest'], 'false')
        self.assertEqual(self.remote_tag, self.ref)

    def test_retry_after_tag_failure_preserves_existing_draft(self):
        self.fail_tag = True
        with self.assertRaisesRegex(RuntimeError, 'Tag creation failed'):
            self.draft()
        self.existing[0]['body'] = 'Edited in GitHub'
        self.fail_tag = False
        self.draft()
        self.assertEqual([call[1] for call in self.writes()],
                         ['releases', 'refs/tags/v0.2.2', 'refs/tags/v0.2.2'])
        self.assertEqual(self.existing[0]['body'], 'Edited in GitHub')
        self.assertEqual(self.remote_tag, self.ref)

    def test_existing_tag_without_release_recovers_draft(self):
        self.remote_tag = self.ref
        self.git('tag', 'v0.2.2')
        self.draft()
        self.assertEqual([call[1] for call in self.writes()], ['releases'])

    def test_no_ff_merge_tags_the_merge_commit(self):
        self.git('reset', '--hard', 'HEAD^')
        self.git('checkout', '-b', 'release')
        branch_commit = self.merge_release()
        self.git('checkout', 'main')
        self.git('merge', '--no-ff', 'release', '-m', 'Merge release PR')
        merge_commit = self.git('rev-parse', 'HEAD')
        self.assertNotEqual(merge_commit, branch_commit)
        self.draft()
        self.assertEqual(self.remote_tag, merge_commit)

    def test_rebase_merge_includes_notes_committed_after_version_bump(self):
        self.changelog.write_text(self.changelog.read_text().replace('Curated release notes.', 'Final curated notes.'))
        merged_commit = self.commit('docs: curate release notes')
        self.merged = [dict(merged_at='2026-11-15T10:00:00Z', merge_commit_sha=merged_commit,
                            base={'ref': 'main'}, head={'ref': 'automation/release'})]
        self.git('commit', '--allow-empty', '-m', 'feat: unrelated later work')
        self.draft()
        self.assertEqual(self.remote_tag, merged_commit)
        self.assertEqual(self.writes()[0][2]['body'], '- Final curated notes.')

    def test_published_release_is_not_modified(self):
        self.remote_tag = self.ref
        self.existing.append(dict(tag_name='v0.2.2', draft=False))
        self.draft()
        self.assertEqual(self.writes(), [])

    def test_conflicting_tag_is_never_moved(self):
        self.remote_tag = 'a' * 40
        with self.assertRaisesRegex(ValueError, 'tag points to another commit'):
            self.draft()
        self.assertEqual(self.writes(), [])

    def test_conflicting_draft_target_is_rejected(self):
        self.existing.append(dict(tag_name='v0.2.2', draft=True, target_commitish='a' * 40))
        with self.assertRaisesRegex(ValueError, 'Draft release targets another commit'):
            self.draft()
        self.assertEqual(self.writes(), [])

    def test_missing_release_notes_prevents_writes(self):
        self.git('reset', '--soft', 'HEAD^')
        self.changelog.write_text('# Changelog\n\n## [Unreleased]\n\n' + self.history)
        self.commit('chore(release): prepare v0.2.2')
        with self.assertRaisesRegex(ValueError, 'Missing changelog notes'):
            self.draft()
        self.assertEqual(self.writes(), [])

    def test_older_workspace_version_is_rejected(self):
        self.git('tag', 'v0.3.0')
        with self.assertRaisesRegex(ValueError, 'older than the latest stable tag'):
            self.draft()
        self.assertEqual(self.writes(), [])


if __name__ == '__main__':
    unittest.main()
