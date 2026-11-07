#!/usr/bin/env python3
"""Build the website and documentation snapshots using Python 3.12 or newer."""

import io
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path
from urllib.request import Request, urlopen

BOOK = Path(__file__).resolve().parent
REPO = BOOK.parent


def git(*args):
    return subprocess.check_output(["git", "-c", "core.fsmonitor=false", *args], cwd=REPO)


def release_versions():
    url = "https://api.github.com/repos/mitos-editor/mitos/releases?per_page=100"
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "mitos-docs"}
    if token := os.environ.get("GITHUB_TOKEN"):
        headers["Authorization"] = f"Bearer {token}"
    tags = set()
    while url:
        with urlopen(Request(url, headers=headers), timeout=30) as response:
            for release in json.load(response):
                tag = release["tag_name"]
                if (not release["draft"] and not release["prerelease"]
                        and re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag)):
                    tags.add(tag)
            next_page = re.search(r'<([^>]+)>; rel="next"', response.headers.get("Link", ""))
            url = next_page[1] if next_page else None
    if not tags:
        raise ValueError("No published stable releases found")
    return sorted(tags, key=lambda tag: tuple(map(int, tag[1:].split("."))), reverse=True)


def build_docs(source, output, version, versions):
    # Keep historical content and generated tables, with the current site presentation.
    for name in ["book.toml", "custom.css", "theme", "ts-query.js", "page-toc.js", "doc-versions.js"]:
        target = source / name
        if target.is_dir():
            shutil.rmtree(target)
        elif target.exists():
            target.unlink()
        origin = BOOK / name
        if origin.is_dir():
            shutil.copytree(origin, target)
        else:
            shutil.copy2(origin, target)
    config = source / "book.toml"
    source_link = "/edit/main/" if version == "main" else f"/blob/{version}/"
    config.write_text(config.read_text().replace("/edit/main/", source_link))
    options = []
    for tag in [*versions, "main"]:
        label = tag
        if tag == versions[0]:
            label += " (latest)"
        elif tag == "main":
            label += " (development)"
        selected = ' selected' if tag == version else ''
        options.append(f'<option value="{tag}"{selected}>{label}</option>')
    theme = source / "theme/index.hbs"
    selector = (
        '<select id="doc-version" aria-label="Documentation version">'
        + ''.join(options) + '</select>'
    )
    theme.write_text(theme.read_text().replace("<!-- Documentation versions -->", selector))
    subprocess.run(["mdbook", "build", str(source), "--dest-dir", str(output)], check=True)
    return output / "html"


def main():
    versions = release_versions()
    site = BOOK / "site"
    with tempfile.TemporaryDirectory(prefix="mitos-docs-") as tmp:
        tmp = Path(tmp)
        builds = {}
        for version in [*versions, "main"]:
            source = tmp / version / "book"
            source.parent.mkdir()
            if version == "main":
                shutil.copytree(BOOK / "src", source / "src")
            else:
                ref = f"refs/tags/{version}"
                present = subprocess.run(
                    ["git", "-c", "core.fsmonitor=false", "rev-parse", "--verify", ref],
                    cwd=REPO, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                )
                if present.returncode:
                    git("fetch", "--depth=1", "origin", f"{ref}:{ref}")
                with tarfile.open(fileobj=io.BytesIO(git("archive", ref, "book"))) as archive:
                    archive.extractall(source.parent, filter="data")
            builds[version] = build_docs(source, tmp / version / "output", version, versions)
        if site.exists():
            shutil.rmtree(site)
        shutil.copytree(builds[versions[0]], site / "docs")
        manifest = {}
        for version, output in builds.items():
            shutil.copytree(output, site / "docs" / version)
            manifest[version] = sorted(
                page.relative_to(output).as_posix() for page in output.rglob("*.html")
            )
        (site / "docs/versions.json").write_text(json.dumps(manifest, sort_keys=True) + "\n")
    for name in ["index.html", "landing.css", "404.html"]:
        shutil.copy2(BOOK / "homepage" / name, site / name)
    for name in ["favicon.svg", "favicon.png"]:
        shutil.copy2(BOOK / "theme" / name, site / name)
    for name in ["install.sh", "install.ps1", "_headers"]:
        shutil.copy2(BOOK / "install" / name, site / name)
    shutil.copytree(BOOK / "install/checksums", site / "checksums")


if __name__ == "__main__":
    main()
