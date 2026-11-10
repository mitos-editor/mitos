"""Release API tests; no network requests or published releases."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('nightly', ROOT / '.github/scripts/nightly.py')
nightly = importlib.util.module_from_spec(spec)
spec.loader.exec_module(nightly)
REF = 'abcdef123456' + '0' * 28
TAG = 'nightly-2026-11-09-abcdef123456'
REPO = 'example/mitos'
list_releases = nightly.releases


def release(tag=TAG, draft=False, prerelease=True, date='2026-11-09'):
    return dict(id=42, tag_name=tag, draft=draft, prerelease=prerelease, published_at=date)


class NightlyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.output = Path(self.temp.name) / 'output'
        self.output.touch()
        self.env = patch.dict(os.environ, GITHUB_OUTPUT=str(self.output), GITHUB_SHA=REF, GITHUB_RUN_ID='1234', NIGHTLY_REF=REF, NIGHTLY_TAG=TAG)
        self.env.start()
        self.addCleanup(self.env.stop)
        self.calls = []
        self.existing = []
        self.after = [release()]
        self.created_at = '2026-11-09T02:00:00Z'
        self.tag_ref = REF
        self.failure = None
        for name, effect in [('api', self.api), ('releases', [self.existing, self.after]), ('gh', self.gh)]:
            patcher = patch.object(nightly, name, side_effect=effect)
            patcher.start()
            self.addCleanup(patcher.stop)

    def api(self, path, *, method='GET', **fields):
        self.calls.append((method, path, fields))
        if path.endswith('/actions/runs/1234'):
            return dict(created_at=self.created_at)
        if '/git/matching-refs/' in path:
            return []
        if path.endswith('/git/refs'):
            return {}
        if '/commits/nightly-' in path:
            return dict(sha=self.tag_ref)
        if method == 'POST':
            return release(draft=True)
        return {}

    def gh(self, *args):
        self.calls.append(args)
        if args[:2] == ('release', 'upload') and self.failure:
            raise self.failure
        return ''

    def publish(self):
        with patch.object(nightly.Path, 'iterdir', return_value=[Path('dist/SHA256SUMS'), Path('dist/binary')]), \
             patch.object(nightly.Path, 'is_file', return_value=True), \
             patch.object(nightly.subprocess, 'run'):
            nightly.publish(REPO)

    def test_prepares_run_commit_and_original_date_for_retries(self):
        nightly.prepare(REPO)
        self.assertEqual(self.output.read_text(), f'ref={REF}\ntag={TAG}\n')
        self.assertEqual(self.calls, [('GET', f'repos/{REPO}/actions/runs/1234', {})])

    def test_same_commit_gets_a_new_build_each_day(self):
        self.existing.append(release())
        nightly.prepare(REPO)
        self.created_at = '2026-11-10T02:00:00Z'
        nightly.prepare(REPO)
        self.assertEqual(self.output.read_text().splitlines(), [
            f'ref={REF}', f'tag={TAG}', f'ref={REF}',
            f'tag=nightly-2026-11-10-{REF[:12]}'])

    def test_checksum_failure_does_not_create_or_publish_release(self):
        with patch.object(nightly.Path, 'iterdir', return_value=[Path('dist/SHA256SUMS')]), \
             patch.object(nightly.Path, 'is_file', return_value=True), \
             patch.object(nightly.subprocess, 'run', side_effect=subprocess.CalledProcessError(1, 'sha256sum')):
            with self.assertRaises(subprocess.CalledProcessError):
                nightly.publish(REPO)
        self.assertEqual(self.calls, [])

    def test_uploads_before_publishing_and_never_sets_latest(self):
        self.publish()
        upload = next(i for i, c in enumerate(self.calls) if c[:2] == ('release', 'upload'))
        publish = next(i for i, c in enumerate(self.calls) if c[0] == 'PATCH')
        self.assertLess(upload, publish)
        self.assertEqual(self.calls[publish][2], dict(draft=False, prerelease=True, make_latest='false'))
        create = next(c for c in self.calls if c[0] == 'POST' and c[1].endswith('/releases'))
        self.assertIs(create[2]['draft'], True)
        self.assertEqual(create[2]['target_commitish'], REF)

    def test_upload_failure_leaves_draft_and_does_not_prune(self):
        self.failure = subprocess.CalledProcessError(1, ['gh', 'release', 'upload'])
        with self.assertRaises(subprocess.CalledProcessError):
            self.publish()
        self.assertFalse(any(c[0] == 'PATCH' or c[:2] == ('release', 'delete') for c in self.calls))

    def test_published_release_is_not_modified(self):
        self.existing.append(release())
        self.publish()
        self.assertEqual(self.calls, [])

    def test_retry_reuses_draft(self):
        self.existing.append(release(draft=True))
        self.publish()
        self.assertFalse(any('POST' in c and c[0].endswith('/releases') for c in self.calls))
        self.assertTrue(any(c[0] == 'PATCH' for c in self.calls))

    def test_wrong_tag_commit_is_rejected(self):
        self.tag_ref = '0' * 40
        with self.assertRaisesRegex(ValueError, 'another commit'):
            self.publish()
        self.assertFalse(any(c[:2] == ('release', 'upload') for c in self.calls))

    def test_retention_only_deletes_old_dated_nightlies(self):
        self.after[:] = [release(tag=f'nightly-2026-10-{day:02}-abcdef123456', date=f'2026-10-{day:02}')
                         for day in range(1, 17)]
        self.after += [release('v0.2.1', prerelease=False), release('v0.3.0-rc1'), release(draft=True),
                       release('nightly-2026-09-01-abcdef123456', date='2026-11-12')]
        self.publish()
        deleted = [c[2] for c in self.calls if c[:2] == ('release', 'delete')]
        self.assertEqual(deleted, ['nightly-2026-10-02-abcdef123456',
                                   'nightly-2026-10-01-abcdef123456',
                                   'nightly-2026-09-01-abcdef123456'])
        self.assertTrue(all('--cleanup-tag' in c for c in self.calls if c[:2] == ('release', 'delete')))

    def test_release_listing_paginates(self):
        with patch.object(nightly, 'gh', return_value=json.dumps([[release()], [release('v0.2.1')]])) as gh:
            self.assertEqual(len(list_releases(REPO)), 2)
            self.assertIn('--paginate', gh.call_args.args)


class GithubApiTests(unittest.TestCase):
    @patch.object(nightly.subprocess, 'check_output', return_value='{"id": 42}\n')
    def test_sends_json_fields_with_boolean_types(self, request):
        result = nightly.api('repos/example/mitos/releases', method='POST',
                             draft=True, prerelease=True, make_latest='false')
        self.assertEqual(result, {'id': 42})
        self.assertEqual(request.call_args.args[0], [
            'gh', 'api', 'repos/example/mitos/releases', '--method', 'POST', '--input', '-'])
        self.assertEqual(json.loads(request.call_args.kwargs['input']),
                         dict(draft=True, prerelease=True, make_latest='false'))

    @patch.object(nightly.subprocess, 'check_output', return_value='{"sha": "abcdef"}')
    def test_reads_do_not_send_a_request_body(self, request):
        self.assertEqual(nightly.api('repos/example/mitos/commits/main'), {'sha': 'abcdef'})
        self.assertEqual(request.call_args.args[0], [
            'gh', 'api', 'repos/example/mitos/commits/main', '--method', 'GET'])
        self.assertIsNone(request.call_args.kwargs['input'])


if __name__ == '__main__':
    unittest.main()
