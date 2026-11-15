"""Prepare one Mitos release PR, then create its draft release and version tag."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib


VERSION = re.compile(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)')


def run(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


def version_tuple(version):
    if not VERSION.fullmatch(version):
        raise ValueError(f'Not a stable release version: {version}')
    return tuple(map(int, version.split('.')))


def workspace_version(text):
    version = tomllib.loads(text)['workspace']['package']['version']
    version_tuple(version)
    return version


def baseline():
    tags = [tag for tag in run('git', 'tag', '--merged', 'HEAD').splitlines()
            if tag.startswith('v') and VERSION.fullmatch(tag[1:])]
    if not tags:
        raise ValueError('No stable Mitos version tag is reachable from HEAD')
    return max(tags, key=lambda tag: version_tuple(tag[1:]))


def release_notes(changelog, version):
    entry = re.search(
        rf'^## \[{re.escape(version)}\][^\n]*\n(.*?)(?=^## |\Z)',
        changelog, re.MULTILINE | re.DOTALL,
    )
    if not entry or not entry[1].strip():
        raise ValueError(f'Missing changelog notes for {version}')
    return entry[1].strip()


def prepare():
    manifest = Path('Cargo.toml')
    original = manifest.read_text()
    current = workspace_version(original)
    previous = baseline()
    if current != previous[1:]:
        raise ValueError('Create the merged version draft before preparing another release')
    if not run('git', 'rev-list', f'{previous}..HEAD'):
        return None
    cliff = ['git-cliff', '--config', 'cliff.toml', '--unreleased', f'{previous}..HEAD']
    tag = run(*cliff, '--bumped-version')
    version_tuple(tag.removeprefix('v'))
    if not tag.startswith('v') or version_tuple(tag[1:]) <= version_tuple(current):
        return None
    generated = run(*cliff, '--tag', tag)
    # Keep handwritten notes and all historical release entries verbatim.
    changelog = Path('CHANGELOG.md')
    parts = re.fullmatch(
        r'(.*?^## \[Unreleased\][^\n]*\n)(.*?)(^## .*|\Z)',
        changelog.read_text(), re.MULTILINE | re.DOTALL,
    )
    if not parts:
        raise ValueError('CHANGELOG.md needs an Unreleased section')
    heading, body = generated.split('\n', 1)
    notes = '\n\n'.join(part.strip() for part in (parts[2], body) if part.strip())
    updated_changelog = f'{parts[1].rstrip()}\n\n{heading}\n\n{notes}\n\n{parts[3]}'
    section = re.search(r'^\[workspace\.package\]\n.*?(?=^\[|\Z)',
                        original, re.MULTILINE | re.DOTALL)
    if not section:
        raise ValueError('Missing workspace.package section')
    updated_section, count = re.subn(r'^version\s*=\s*"[^"]+"', f'version = "{tag[1:]}"',
                                     section[0], count=1, flags=re.MULTILINE)
    if count != 1:
        raise ValueError('Missing workspace.package.version')
    manifest.write_text(original[:section.start()] + updated_section + original[section.end():])
    # Cargo owns the lockfile. --workspace retains locked external dependencies.
    subprocess.run(['cargo', 'update', '--workspace'], check=True)
    changelog.write_text(updated_changelog)
    return tag[1:]


def api(repo, path, *, method='GET', **fields):
    args = ['gh', 'api', f'repos/{repo}/{path}', '--method', method]
    if fields:
        args += ['--input', '-']
    return json.loads(run(*args, input=json.dumps(fields) if fields else None))


def draft(repo):
    version = workspace_version(Path('Cargo.toml').read_text())
    if version_tuple(version) < version_tuple(baseline()[1:]):
        raise ValueError('Workspace version is older than the latest stable tag')
    tag = f'v{version}'
    # Tag the version bump commit even if more commits reached main before this run.
    commit = run('git', 'log', '--first-parent', '-1', '--format=%H',
                 '-G', rf'^version = "{re.escape(version)}"$', '--', 'Cargo.toml')
    if not commit or workspace_version(run('git', 'show', f'{commit}:Cargo.toml')) != version:
        raise ValueError('Cannot find the release version commit')
    # A rebase merge can put curated notes after the original version bump.
    pulls = api(repo, f'commits/{commit}/pulls')
    merged = [pr for pr in pulls if pr['merged_at'] and pr['base']['ref'] == 'main'
              and (pr['head']['ref'] == 'automation/release'
                   or pr['head']['ref'].startswith('release-plz-'))]
    if merged:
        commit = max(merged, key=lambda pr: pr['merged_at'])['merge_commit_sha']
        subprocess.run(['git', 'merge-base', '--is-ancestor', commit, 'HEAD'], check=True)
        if workspace_version(run('git', 'show', f'{commit}:Cargo.toml')) != version:
            raise ValueError('Merged release PR has another version')
    refs = api(repo, f'git/matching-refs/tags/{tag}')
    tagged = any(ref['ref'] == f'refs/tags/{tag}' for ref in refs)
    if tagged and api(repo, f'commits/{tag}')['sha'] != commit:
        raise ValueError('Release tag points to another commit')
    pages = json.loads(run('gh', 'api', f'repos/{repo}/releases?per_page=100',
                           '--paginate', '--slurp'))
    release = next((r for page in pages for r in page if r['tag_name'] == tag), None)
    if release and not release['draft']:
        print(f'{tag} is already published; leaving it unchanged.')
        return
    if release and not tagged and release['target_commitish'] != commit:
        raise ValueError('Draft release targets another commit')
    if release is None:
        notes = release_notes(run('git', 'show', f'{commit}:CHANGELOG.md'), version)
        api(repo, 'releases', method='POST', tag_name=tag, target_commitish=commit,
            name=f'Mitos {version}', body=notes, draft=True, prerelease=False, make_latest='false')
    # The draft must exist before this tag starts the packaging workflow.
    if not tagged:
        run('git', '-c', 'credential.helper=', '-c',
            'credential.helper=!gh auth git-credential', 'push',
            f'https://github.com/{repo}.git', f'{commit}:refs/tags/{tag}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['prepare', 'draft'])
    args = parser.parse_args()
    if args.command == 'draft':
        draft(os.environ['GITHUB_REPOSITORY'])
    else:
        version = prepare()
        output = f'changed={str(version is not None).lower()}\nversion={version or ""}\n'
        if path := os.environ.get('GITHUB_OUTPUT'):
            with open(path, 'a') as file:
                file.write(output)
        print(output, end='')


if __name__ == '__main__':
    main()
