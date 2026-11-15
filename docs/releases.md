# Releases

[git-cliff](https://git-cliff.org/) prepares one changelog for the entire repository.
The Prepare release workflow opens a release PR on pushes to `main` when none is
already open. Merging that PR creates a draft GitHub release and a version tag such as `v0.2.1`;
the tag triggers `.github/workflows/release.yml` to build and upload the binary
archives, Linux AppImages, Debian package, SHA-256 checksums, and provenance attestations.
The release stays in draft until you publish it manually. Artifact uploads find
the existing draft by ID and refuse to proceed if it is already published.

Mitos has one application version, one root changelog, and one version tag.
The version comes from `workspace.package.version`; internal crates inherit it.
Independently versioned crates, including the vendored `crates/lsp-types`, keep
their versions. Release preparation regenerates `Cargo.lock` with
`cargo update --workspace`, retaining locked external dependencies. No crates
are published to crates.io.

## Version baseline

Mitos uses semantic versions starting at **0.1.0**, independently of Helix's
release history. Tags use the form `v0.1.0`. Inherited Helix tags must not be
pushed back into this repository.

Release preparation starts at the highest stable `vX.Y.Z` tag reachable from
`main`. Nightly, prerelease, and unrelated upstream tags are ignored. The
repository must already have a stable version tag.

`cliff.toml` defines the notes and version policy. Below 1.0, features and fixes
suggest a patch bump; breaking changes suggest a minor bump. Commits anywhere
in the repository contribute, including grammars, themes, and packaging.
Existing `Unreleased` notes move into the new release entry, and historical
entries are preserved. Review and curate the generated notes before merging.

An open release PR is left unchanged so manual version choices and curated notes
are preserved. This includes existing release-plz PRs during the migration.
Merge current `main` into the release branch and update its notes if more changes
need to be included before releasing. After a release PR is merged, the workflow
creates the draft before pushing the tag that starts packaging. Retries reuse
the draft and never move an existing tag or edit a published release.

## Repository setup

Create a fine-grained personal access token (PAT) for `mitos-editor/mitos`
from an account with write access to the repository. Grant repository
**Contents: read and write** and **Pull requests: read and write** permissions,
and save the token as the repository Actions secret `RELEASE_PAT`.

The PAT allows release PRs to trigger CI and version tags to trigger the binary
build workflow. The Prepare release workflow uses `RELEASE_PAT` for both operations;
checkout credentials remain disabled.

## Release checklist

1. Review the release PR's version, `Cargo.lock`, and `CHANGELOG.md`. Editor
   packages must share `workspace.package.version`; keep the vendored `lsp-types`
   version and external dependency versions unchanged. For a minor release such
   as 0.2.0, set that version in `Cargo.toml` and regenerate the lockfile with Cargo.
2. Curate the changelog in plain English, including runtime, grammar, theme, and
   packaging changes outside Rust packages. Link feature documentation and
   upstream libraries, and explain breaking changes and how to upgrade.
3. Add a release entry to `contrib/Mitos.appdata.xml` following the
   [AppStream release metadata specification](https://www.freedesktop.org/software/appstream/docs/sect-Metadata-Releases.html).
   This metadata and the desktop icon are included in Debian, AppImage, and Nix
   packages. Nix reads its version from the workspace manifest.
4. Run `python3 -m unittest discover -s tests/installers -v`, build the book,
   and validate release workflow changes with `actionlint`. Check
   `nix/grammar-sources.json` against `languages.toml`; regenerate changed pins
   with `cargo xtask update-nix-grammar-sources` on a machine with Nix installed.
5. Wait for CI and merge the release PR.
6. Verify the Prepare release and Release workflows finish. Check all six platform
   archives, both Linux AppImages, the x86_64 Debian package, source archive,
   `SHA256SUMS`, and provenance attestations on the draft GitHub release. The source
   archive includes pinned grammar sources and excludes Git metadata and CI
   build artifacts. Test `ms --version` and `ms --health rust` from an extracted
   archive outside the checkout. Download draft assets while authenticated with
   `gh release download v0.2.1 --repo mitos-editor/mitos` (replace the version).
7. When the Release workflow is green and the draft has all artifacts, open it
   in [GitHub Releases](https://github.com/mitos-editor/mitos/releases), review
   the notes, and click **Publish release**. Alternatively, run
   `gh release edit v0.2.1 --repo mitos-editor/mitos --draft=false --latest`.
8. Test the hosted installers with the newly published version. Public download
   URLs and the latest-release installer do not expose draft releases; the
   previous published release stays available while the new draft builds.
9. Check that the Stable branch workflow moved `stable` to the latest release commit.

If a build fails, the release stays in draft. Retry the failed jobs before
publishing; do not move the version tag. Uploads and reruns never publish the
release automatically. A rerun after manual publication fails the draft check
rather than changing the published release.

Manual dispatch of the Release workflow remains a preview build and uploads CI
artifacts. Manual dispatch of Prepare release on `main` reruns release automation.

To prepare the same version and changelog locally, install git-cliff 2.14.2 and
the repository's Rust toolchain, start from a clean `main` checkout with tags,
and run:

```sh
python3 .github/scripts/release.py prepare
```

This edits `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` without creating a PR,
tag, or GitHub release. To choose another version, edit
`workspace.package.version`, regenerate the lockfile with `cargo update --workspace`,
and adjust the new changelog heading and comparison link before merging.

## Branches and channels

Use `main` for development and `stable` to follow the latest published stable
release. Version tags never move. Keep `stable` free of direct commits.
There is no separate `develop` branch.

The Stable branch workflow resolves GitHub's **Latest** release, skips drafts
and prereleases, and pushes its commit without force. Delayed events therefore
follow the current release. Updates run one at a time. If the target is not a
descendant of `stable`, the workflow fails without moving the branch. Resolve
that deliberately if releases ever come from separate maintenance branches.

After merging these workflows, run **Stable branch → Run workflow** on `main`
to create `stable`. The same action repairs a missed update. Create the branch
before announcing the stable Nix input. Restrict direct pushes and deletion
with a repository ruleset that allows this workflow to update it.

For a release that needs stabilization, create a temporary `release/0.x` branch
and merge its fixes back into `main`.

## Nightly builds

The Nightly workflow is scheduled daily at 02:00 UTC and can be started manually
on `main`. Every run tests and builds its snapshot of `main`, even if the code
has not changed since the previous night. It reuses the Build workflow before
packaging. A failed test or build prevents publication.

Tags use the run's original UTC date, so retrying a failed run on another day
keeps the same tag.

Stable releases and nightlies use `package-release.yml`, with Rust 1.97.1,
the same six platforms, bundled runtimes, AppImages, Debian package, checksums,
and provenance. Each platform extracts its archive and checks the binary and
runtime outside the checkout. Nightly Debian versions sort below the
corresponding stable version.

Nightlies use dated tags such as `nightly-2026-11-09-abcdef123456`. All builds and
uploads must succeed before publication. Failed uploads leave a draft that a
rerun can resume. Published assets are never replaced. A failed run leaves the
previous nightly available. After publication, cleanup keeps the last 14 nightly
releases and removes their older tags; stable releases and other prereleases
are untouched. Installers and cleanup order nightlies by their tag date, so a
late retry of an older build does not replace a newer day's build. Download a
build if you need to keep it longer.

Installers default to stable. Use `--channel nightly` or `-Channel nightly` to
try a nightly, or pass a dated tag as the version. See the
[installation guide](../book/src/install.md#nightly-builds).

Stable publication is manual. If you automate it later with `GITHUB_TOKEN`,
call the stable update explicitly: its release event will not start another
workflow. Publishing with a GitHub App token or PAT also permits that trigger.

Validate release automation without publishing anything. Install git-cliff
2.14.2 and Cargo to exercise the release preparation fixtures:

```sh
python3 -m unittest discover -s tests/releases -v
python3 -m unittest discover -s tests/installers -v
actionlint
```

## Hosted installers

The website serves `book/install/install.sh` and `book/install/install.ps1`.
Both keep each release's executable and runtime together and verify archives
against the release's `SHA256SUMS` asset. Version `0.1.0` predates that asset;
its checksum manifest is checked into `book/install/checksums/v0.1.0.txt`, using
GitHub's published SHA-256 asset digests, and served at `/checksums/v0.1.0.txt`.

Run the installer regression tests with:

```sh
python3 -m unittest discover -s tests/installers -v
```

PowerShell tests run when `pwsh` is available (or set `PWSH` to its path).
The Installers workflow runs these tests on Linux, macOS, and Windows.
`./book/build-site.sh` copies both installers, their HTTP headers, and the
initial checksum manifest into the generated site for deployment.

The shell installer follows the [rustup bootstrap pattern](https://github.com/rust-lang/rustup/blob/main/rustup-init.sh):
define the installer before calling its entry point at the end, and restrict
downloads to HTTPS. Default Unix locations follow the
[XDG Base Directory Specification](https://specifications.freedesktop.org/basedir/0.8/).
The Windows installer uses PowerShell's standard parameter block, comment-based
help, and an explicit PowerShell 5.1 minimum. Both retain the runtime alongside
the executable rather than installing the binary alone.
