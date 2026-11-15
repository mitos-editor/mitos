# Releases

[git-cliff](https://git-cliff.org/) generates one changelog for the entire
repository. Maintainers prepare and curate a release PR, then create its draft
GitHub release and version tag manually. Pushing a tag such as `v0.2.2` starts
`.github/workflows/release.yml`, which builds and uploads the binary archives,
Linux AppImages, Debian package, SHA-256 checksums, and provenance attestations.
The release stays in draft until a maintainer publishes it. Artifact uploads
find the existing draft by ID and refuse to proceed if it is already published.

Mitos has one application version from `workspace.package.version`; internal
crates inherit it. Independently versioned crates, including the vendored
`crates/lsp-types`, keep their versions. No crates are published to crates.io.

## Prepare a release PR

Install git-cliff 2.14.2, cargo-edit 0.13.14 (which provides `cargo set-version`),
and the repository's Rust toolchain. Start from a clean branch based on current
`main`, with the repository's tags fetched:

```sh
git fetch origin --tags
git cliff --unreleased --bumped-version
```

Choose the version using that suggestion, for example `v0.2.2`, then prepare
the files for review:

```sh
tag=v0.2.2
cargo set-version --package term "${tag#v}"
git cliff --unreleased --tag "$tag" --prepend CHANGELOG.md
```

`cargo set-version` updates the shared workspace version and lockfile while
preserving independently versioned crates. The native `--prepend` operation
keeps historical entries and moves handwritten `Unreleased` notes into the new
release entry alongside the generated notes.

`cliff.toml` includes commits across the repository, including grammars, themes,
and packaging. It considers stable `vX.Y.Z` tags on the current branch and
ignores nightly, prerelease, and unrelated upstream tags. Below 1.0, features
and fixes suggest a patch bump; breaking changes suggest a minor bump. Mitos's
version history is independent of Helix's. Do not push inherited Helix tags back
into this repository.

1. Review `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md`. Editor crates must share
   `workspace.package.version`; keep vendored versions and external dependency
   versions unchanged.
2. Curate the changelog in plain English. Add custom notes, link feature
   documentation and upstream libraries, and explain breaking changes and how
   to upgrade. The generated entries are a starting point.
3. Add a release entry to `contrib/Mitos.appdata.xml` following the
   [AppStream release metadata specification](https://www.freedesktop.org/software/appstream/docs/sect-Metadata-Releases.html).
   This metadata and the desktop icon are included in Debian, AppImage, and Nix
   packages. Nix reads its version from the workspace manifest.
4. Run `python3 -m unittest discover -s tests/installers -v`, build the book,
   and validate release workflow changes with `actionlint`. Check
   `nix/grammar-sources.json` against `languages.toml`; regenerate changed pins
   with `cargo xtask update-nix-grammar-sources` on a machine with Nix installed.
5. Open the release PR, wait for CI, and merge it. Merging does not create a tag
   or release. If release-plz already opened a PR, it can be curated and merged
   manually in the same way.

## Tag and publish

Use the merged release PR's full commit SHA, so later commits on `main` do not
enter the release accidentally. Save the curated version entry from
`CHANGELOG.md`, without its heading or older entries, to
`/tmp/mitos-release-notes.md`.

Set the prepared version and merged commit below, then save the draft release
before pushing the tag:

```sh
tag=v0.2.2
release_commit=FULL_MERGED_RELEASE_COMMIT_SHA
gh release create "$tag" --repo mitos-editor/mitos \
  --target "$release_commit" --title "Mitos ${tag#v}" \
  --notes-file /tmp/mitos-release-notes.md --draft --latest=false
git tag -a "$tag" "$release_commit" -m "Mitos ${tag#v}"
git push origin "refs/tags/$tag"
```

A saved draft does not publish the release. The explicit tag push starts the
Release workflow; the draft must exist before its artifact uploads run. Use
an account with repository write access. Release preparation no longer needs
an automation token; `RELEASE_PAT` may still be used by other workflows such as
grammar updates.

1. Verify the Release workflow finishes. Check all six platform archives, both
   Linux AppImages, the x86_64 Debian package, source archive, `SHA256SUMS`, and
   provenance attestations on the draft release. The source archive includes
   pinned grammar sources and excludes Git metadata and CI build artifacts.
   Test `ms --version` and `ms --health rust` from an extracted archive outside
   the checkout. Download draft assets while authenticated with
   `gh release download v0.2.2 --repo mitos-editor/mitos` (replace the version).
2. Open the draft in [GitHub Releases](https://github.com/mitos-editor/mitos/releases),
   review the notes, and click **Publish release**. Alternatively, run
   `gh release edit v0.2.2 --repo mitos-editor/mitos --draft=false --latest`.
3. Test the hosted installers with the newly published version. Public download
   URLs and the latest-release installer do not expose draft releases; the
   previous published release stays available while the new draft builds.
4. Check that the Stable branch workflow moved `stable` to the latest release commit.

If a build fails, the release stays in draft. Retry the failed jobs before
publishing; do not move the version tag. Uploads and reruns never publish the
release automatically. A rerun after manual publication fails the draft check
rather than changing the published release.

Manual dispatch of the Release workflow remains a preview build and uploads CI
artifacts.

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

Validate the existing release and installer automation without publishing anything:

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
