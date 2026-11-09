# Releases

[release-plz](https://release-plz.dev/) opens and updates a release PR on pushes
to `main`. Merging that PR creates a draft GitHub release and a version tag such as `v0.2.1`;
the tag triggers `.github/workflows/release.yml` to build and upload the binary
archives, Linux AppImages, Debian package, SHA-256 checksums, and provenance attestations.
The release stays in draft until you publish it manually. Artifact uploads find
the existing draft by ID and refuse to proceed if it is already published.

Releases use Git tags only; release-plz does not publish crates to crates.io.
The `term` package owns the release and root changelog. Its version is inherited
from `workspace.package.version`, so the editor's internal workspace crates
remain on the same version. External dependencies, including `lsp-types`, retain
their upstream versions.

## Version baseline

Mitos uses semantic versions starting at **0.1.0**, independently of Helix's
release history. Tags use the form `v0.1.0`. Inherited Helix tags must not be
pushed back into this repository.

With no existing release tags, release-plz treats `0.1.0` as the initial
release and keeps that version in its first release PR. Subsequent versions
follow release-plz's semantic version bumps from commit messages.
`release_always = false` ensures draft release creation happens only after a
release PR is merged. `git_release_draft = true` keeps the release unpublished.

## Repository setup

Create a fine-grained personal access token (PAT) for `mitos-editor/mitos`
from an account with write access to the repository. Grant repository
**Contents: read and write** and **Pull requests: read and write** permissions,
and save the token as the repository Actions secret `RELEASE_PAT`.

The PAT allows release PRs to trigger CI and version tags to trigger the binary
build workflow. See the [release-plz token setup guide](https://release-plz.dev/docs/github/token).

## Release checklist

1. Review the release PR's version, `Cargo.lock`, and `CHANGELOG.md`. All workspace
   packages must share `workspace.package.version`; keep external dependency
   versions unchanged. For a minor release such as 0.2.0, set that version in
   `Cargo.toml` and regenerate the lockfile with Cargo.
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
6. Verify the Release-plz and Release workflows finish. Check all six platform
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

If a build fails, the release stays in draft. Retry the failed jobs before
publishing; do not move the version tag. Uploads and reruns never publish the
release automatically. A rerun after manual publication fails the draft check
rather than changing the published release.

Manual dispatch of the Release workflow remains a preview build and uploads CI
artifacts. Manual dispatch of Release-plz on `main` reruns release automation.

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
