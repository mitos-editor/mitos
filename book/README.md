# Mitos website

The website combines a standalone landing page with the mdBook documentation:

- `homepage/` contains the landing page and its styles.
- `src/` contains the mdBook documentation.
- `install/` contains the hosted shell and PowerShell installers and the checksum manifest for the first release.
- `site/` is the generated website, with the documentation under `site/docs/`.

Build the complete site from the repository root with Python 3.12 or newer, Git,
and mdBook available:

```sh
./book/build-site.sh
```

Serve `book/site/` with any static file server to preview the landing page and
documentation together. Running `mdbook serve book` remains useful when working
only on documentation.

To check local documentation links during `mdbook build` or the site build,
install the mdBook backend:

```sh
cargo +stable install mdbook-linkcheck2 --version 0.12.2 --locked
```

The checker is optional for local and deployment builds and required by the
Book workflow. It checks local file links without contacting external sites;
heading anchors and TOML examples are not checked. With multiple backends,
plain `mdbook build book` writes HTML under `book/book/html/`; the site script
publishes each snapshot under `site/docs/`.

## Documentation versions

`/docs/` serves the latest stable release. Permanent snapshots live at
`/docs/v0.1.2/` (and the other published versions), while `/docs/main/` serves
the documentation from the current checkout. Production builds use `main`;
branch previews use that branch's content for the development snapshot.
The version selector preserves the chapter, query, and anchor when the target
version has that chapter, and otherwise opens its introduction. Search stays
within the selected snapshot.

The build discovers published stable releases from GitHub's public releases API,
excluding drafts, prereleases, and inherited Helix tags. It fetches missing
release tags from `origin`, including in shallow Cloudflare and Actions
checkouts. Each release's Markdown and committed generated tables come from
its tag; the current theme, scripts, and mdBook configuration provide a
consistent presentation. Release source links point at the corresponding tag.
Do not regenerate historical tables from the current Rust sources.

A full site build needs network access to GitHub. `GITHUB_TOKEN` is optional
and increases the API rate limit; the Book workflow uses its read-only token.
For offline editing, use `mdbook serve book`.

The Rebuild documentation workflow triggers a Cloudflare build when GitHub
releases are published, edited, deleted, or unpublished, and supports manual
dispatch. Configure it once:

1. In the `mitos` Worker's **Settings > Builds > Deploy Hooks**, create a hook
   for the `main` branch.
2. Save its URL as the GitHub repository Actions secret
   `CLOUDFLARE_DEPLOY_HOOK`. Treat the URL as a credential.

See [Cloudflare's deploy hook documentation](https://developers.cloudflare.com/workers/ci-cd/builds/deploy-hooks/).
The workflow only starts the build; check Workers Builds for deployment status.
Normal pushes continue to use the existing Cloudflare Git integration.

Run the focused regression checks with:

```sh
python3 -m unittest discover -s tests/docs -v
node --test tests/docs/test_versions.cjs
```

`mdbook serve book` previews the current documentation without the multi-version
selector; use a complete site build to preview version switching.

Command and language tables under `src/generated/` come from `cargo xtask docgen`;
edit their Rust metadata or `languages.toml` source before regenerating them.

The site build also publishes `/install.sh`, `/install.ps1`, and `/checksums/`.
Installers download binaries from GitHub Releases. New releases include
`SHA256SUMS`; `0.1.0` uses the checked-in manifest from GitHub's asset digests.

## Cloudflare Workers

The site is configured as a static-assets Worker in `wrangler.jsonc`. To build,
preview, or deploy it locally:

```sh
cd book
./cloudflare-build.sh
wrangler dev
wrangler deploy
```

This assumes `wrangler` is installed separately; the repository has no
JavaScript package manifest or dependencies.

`cloudflare-build.sh` uses an installed `mdbook` when available. In Cloudflare's
build environment it downloads the pinned mdBook 0.5.3 release binary before
assembling `site/`.

For a Workers Builds GitHub integration, use these settings:

- Production branch: `main`
- Root directory: `book`
- Build command: `./cloudflare-build.sh`
- Deploy command: `npx wrangler deploy`
- Non-production branch deploy command: `npx wrangler versions upload`

The `mitos.computer` zone must be active in the same Cloudflare account and the
hostname must not already have a conflicting CNAME record. Wrangler will create
the Custom Domain DNS record and certificate during the first production deploy.
