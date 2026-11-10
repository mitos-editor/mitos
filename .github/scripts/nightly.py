"""Prepare and publish complete, dated nightly releases."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess


def gh(*args, payload=None):
    return subprocess.check_output(['gh', *args], input=payload, text=True).strip()


def api(path, *, method='GET', **fields):
    args = ['api', path, '--method', method]
    if fields:
        args += ['--input', '-']
    payload = json.dumps(fields) if fields else None
    return json.loads(gh(*args, payload=payload))


def releases(repo):
    pages = json.loads(gh('api', f'repos/{repo}/releases?per_page=100', '--paginate', '--slurp'))
    return [release for page in pages for release in page]


def is_nightly(release):
    return (
        release['prerelease'] and not release['draft']
        and re.fullmatch(r'nightly-\d{4}-\d{2}-\d{2}-[0-9a-f]{12}', release['tag_name']) is not None
    )


def prepare(repo):
    ref = os.environ['GITHUB_SHA']
    # Keep the original date when a failed run is retried on another day.
    run = api(f'repos/{repo}/actions/runs/{os.environ["GITHUB_RUN_ID"]}')
    date = run['created_at'][:10]
    tag = f'nightly-{date}-{ref[:12]}'
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        output.write(f'ref={ref}\ntag={tag}\n')


def publish(repo):
    ref, tag = os.environ['NIGHTLY_REF'], os.environ['NIGHTLY_TAG']
    if not re.fullmatch(r'[0-9a-f]{40}', ref) or not re.fullmatch(
            rf'nightly-\d{{4}}-\d{{2}}-\d{{2}}-{ref[:12]}', tag):
        raise ValueError('Invalid nightly commit or tag')
    assets = sorted(Path('dist').iterdir())
    if not assets or not Path('dist/SHA256SUMS').is_file():
        raise ValueError('Missing release assets or SHA256SUMS')
    # Reject incomplete or modified downloads before touching a release.
    subprocess.run(['sha256sum', '--check', 'SHA256SUMS'], cwd='dist', check=True)
    release = next((r for r in releases(repo) if r['tag_name'] == tag), None)
    if release and not release['draft']:
        print('Nightly is already published; leaving its assets unchanged.')
        prune(repo)
        return
    refs = api(f'repos/{repo}/git/matching-refs/tags/{tag}')
    if not any(r['ref'] == f'refs/tags/{tag}' for r in refs):
        api(f'repos/{repo}/git/refs', method='POST', ref=f'refs/tags/{tag}', sha=ref)
    # A pre-existing tag must never silently point at a different build.
    if api(f'repos/{repo}/commits/{tag}')['sha'] != ref:
        raise ValueError('Nightly tag points to another commit')
    if release is None:
        release = api(
            f'repos/{repo}/releases', method='POST',
            tag_name=tag, target_commitish=ref, name=f'Mitos {tag}',
            draft=True, prerelease=True, make_latest='false',
            body=f'Development build from {ref}. May contain bugs. Use a stable release for everyday work.',
        )
    gh('release', 'upload', tag, *(str(p) for p in assets), '--clobber', '--repo', repo)
    api(f'repos/{repo}/releases/{release["id"]}', method='PATCH',
        draft=False, prerelease=True, make_latest='false')
    prune(repo)


def prune(repo):
    # Prune only our dated, published prereleases, after the new one is public.
    nightlies = sorted((r for r in releases(repo) if is_nightly(r)),
                       key=lambda r: (r['tag_name'].rsplit('-', 1)[0], r['published_at']), reverse=True)
    for old in nightlies[14:]:
        gh('release', 'delete', old['tag_name'], '--cleanup-tag', '--yes', '--repo', repo)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['prepare', 'publish'])
    args = parser.parse_args()
    repo = os.environ['GITHUB_REPOSITORY']
    if args.command == 'prepare':
        prepare(repo)
    else:
        publish(repo)


if __name__ == '__main__':
    main()
