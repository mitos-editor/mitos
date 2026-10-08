# Mitos website

The website combines a standalone landing page with the mdBook documentation:

- `homepage/` contains the landing page and its styles.
- `src/` contains the mdBook documentation.
- `install/` contains the hosted shell and PowerShell installers and the checksum manifest for the first release.
- `site/` is the generated website, with the documentation under `site/docs/`.

Build the complete site from the repository root:

```sh
./book/build-site.sh
```

Serve `book/site/` with any static file server to preview the landing page and
documentation together. Running `mdbook serve book` remains useful when working
only on documentation.

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
