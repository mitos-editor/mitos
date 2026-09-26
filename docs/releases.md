# Releases

[release-plz](https://release-plz.dev/) opens and updates a release PR on pushes
to `main`. Merging that PR creates a GitHub release and a version tag such as `v0.1.0`;
the tag triggers `.github/workflows/release.yml` to build and upload the binary
archives, Debian package, and provenance attestations.

Releases use Git tags only; release-plz does not publish crates to crates.io.
The `term` package owns the release and root changelog. Its version is inherited
from `workspace.package.version`, so the editor's internal workspace crates
remain on the same version. `lsp-types` retains its independent vendored version.

## Version baseline

Mitos uses semantic versions starting at **0.1.0**, independently of Helix's
release history. Tags use the form `v0.1.0`. Inherited Helix tags must not be
pushed back into this repository.

With no existing release tags, release-plz treats `0.1.0` as the initial
release and keeps that version in its first release PR. Subsequent versions
follow release-plz's semantic version bumps from commit messages.
`release_always = false` ensures publication happens only after a release PR
is merged.

## Repository setup

Create a fine-grained personal access token (PAT) for `mitos-editor/mitos`
from an account with write access to the repository. Grant repository
**Contents: read and write** and **Pull requests: read and write** permissions,
and save the token as the repository Actions secret `RELEASE_PAT`.

The PAT allows release PRs to trigger CI and version tags to trigger the binary
build workflow. See the [release-plz token setup guide](https://release-plz.dev/docs/github/token).

## Release checklist

1. Review the release PR's version, `Cargo.lock`, and `CHANGELOG.md`. Curate the
   notes, including runtime, grammar, and theme changes outside Rust packages.
2. Add a release entry to `contrib/Mitos.appdata.xml` following the
   [AppStream release metadata specification](https://www.freedesktop.org/software/appstream/docs/sect-Metadata-Releases.html).
3. Wait for CI and merge the release PR.
4. Verify the Release-plz and Release workflows finish, and check the uploaded
   archives, Debian package, and provenance attestations on the GitHub release.

Manual dispatch of the Release workflow remains a preview build and uploads CI
artifacts. Manual dispatch of Release-plz on `main` reruns release automation.
