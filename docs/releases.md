# Release checklist

Mitos uses calendar versions in the form `YY.0M(.MICRO)`. Cargo package
versions must use the semver-compatible equivalent, such as `26.8.0` for an
August 2026 release.

Before tagging a release:

1. Update `workspace.package.version` in `Cargo.toml`.
2. Run `cargo check --workspace --all-targets` and commit `Cargo.lock`.
3. Add release notes to `CHANGELOG.md`.
4. Add a release entry to `contrib/Mitos.appdata.xml` following the
   [AppStream release metadata specification](https://www.freedesktop.org/software/appstream/docs/sect-Metadata-Releases.html).
5. Tag the release and push the tag.
6. Verify the release workflow and its provenance attestations.
7. Publish the generated archives and Debian package from the GitHub release.

Use GitHub's compare view to curate release notes:

```text
https://github.com/mitos-editor/mitos/compare/<previous-tag>...<new-tag>
```
