"""Prepare and publish complete, dated nightly releases."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys


def gh(*args):
    return subprocess.check_output(['gh', *args], text=True).strip()


def api(path, *args):
    return json.loads(gh('api', path, *args))


def releases(repo):
    pages = json.loads(gh('api', f'repos/{repo}/releases?per_page=100', '--paginate', '--slurp'))
    return [release for page in pages for release in page]


def is_nightly(release):
    return (release['prerelease'] and not release['draft']
            and re.fullmatch(r'nightly-\d{4}-\d{2}-\d{2}-[0-9a-f]{12}', release['tag_name']))


def select(repo):
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
        api(f'repos/{repo}/git/refs', '-X', 'POST', '-f', f'ref=refs/tags/{tag}', '-f', f'sha={ref}')
    # A pre-existing tag must never silently point at a different build.
    if api(f'repos/{repo}/commits/{tag}')['sha'] != ref:
        raise ValueError('Nightly tag points to another commit')
    if release is None:
        release = api(f'repos/{repo}/releases', '-X', 'POST',
                      '-f', f'tag_name={tag}', '-f', f'target_commitish={ref}',
                      '-f', f'name=Mitos {tag}', '-F', 'draft=true', '-F', 'prerelease=true',
                      '-f', 'make_latest=false', '-f',
                      f'body=Development build from {ref}. May contain bugs. Use a stable release for everyday work.')
    gh('release', 'upload', tag, *(str(p) for p in assets), '--clobber', '--repo', repo)
    api(f'repos/{repo}/releases/{release["id"]}', '-X', 'PATCH',
        '-F', 'draft=false', '-F', 'prerelease=true', '-f', 'make_latest=false')
    prune(repo)


def prune(repo):
    # Prune only our dated, published prereleases, after the new one is public.
    nightlies = sorted((r for r in releases(repo) if is_nightly(r)),
                       key=lambda r: (r['tag_name'][8:18], r['published_at']), reverse=True)
    for old in nightlies[14:]:
        gh('release', 'delete', old['tag_name'], '--cleanup-tag', '--yes', '--repo', repo)


if __name__ == '__main__':
    {'select': select, 'publish': publish}[sys.argv[1]](os.environ['GITHUB_REPOSITORY'])
