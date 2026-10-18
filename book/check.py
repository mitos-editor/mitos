#!/usr/bin/env python3
"""Check book examples and local links after running book/build-site.sh."""

import argparse
from html.parser import HTMLParser
from pathlib import Path
import re
import textwrap
import tomllib
from urllib.parse import unquote, urlsplit


BOOK = Path(__file__).resolve().parent
REPOSITORY = BOOK.parent


class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.anchors = set()
        self.links = []

    def handle_starttag(self, tag, attributes):
        attributes = dict(attributes)
        if "id" in attributes:
            self.anchors.add(attributes["id"])
        for name in ("href", "src"):
            if name in attributes:
                self.links.append((self.getpos()[0], attributes[name]))


def check_examples():
    errors = []
    count = 0
    pattern = re.compile(r"^[ \t]*```toml[^\S\n]*\n(.*?)^[ \t]*```[^\S\n]*$", re.M | re.S)
    for path in sorted((BOOK / "src").rglob("*.md")):
        source = path.read_text(encoding="utf-8")
        for match in pattern.finditer(source):
            count += 1
            try:
                tomllib.loads(textwrap.dedent(match[1]))
            except tomllib.TOMLDecodeError as error:
                line = source.count("\n", 0, match.start()) + 1
                errors.append(f"{path.relative_to(REPOSITORY)}:{line}: {error}")
    return count, errors


def check_links(site):
    site = site.resolve()
    pages = {}
    errors = []
    count = 0
    for path in sorted(site.rglob("*.html")):
        page = Page()
        page.feed(path.read_text(encoding="utf-8"))
        pages[path] = page

    for path, page in pages.items():
        for line, link in page.links:
            url = urlsplit(link)
            if url.netloc == "github.com":
                for prefix in ("/mitos-editor/mitos/blob/main/", "/mitos-editor/mitos/tree/main/"):
                    if url.path.startswith(prefix):
                        count += 1
                        target = REPOSITORY / unquote(url.path.removeprefix(prefix))
                        if not target.exists():
                            errors.append(f"{path.relative_to(site)}:{line}: missing source: {link}")
                continue
            if url.netloc == "mitos.computer":
                target = site / unquote(url.path).lstrip("/")
            elif url.scheme or url.netloc:
                continue
            elif url.path.startswith("/"):
                target = site / unquote(url.path).lstrip("/")
            else:
                target = path.parent / unquote(url.path) if url.path else path
            target = target.resolve()
            if target.is_dir():
                target /= "index.html"
            count += 1
            if not target.exists():
                errors.append(f"{path.relative_to(site)}:{line}: missing file: {link}")
            elif url.fragment and target in pages:
                if unquote(url.fragment) not in pages[target].anchors:
                    errors.append(f"{path.relative_to(site)}:{line}: missing anchor: {link}")
    return count, errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--site-dir", type=Path, default=BOOK / "site")
    args = parser.parse_args()
    site = args.site_dir.resolve()
    if not (site / "docs/index.html").is_file():
        parser.error("build the full site with ./book/build-site.sh before checking")
    examples, example_errors = check_examples()
    links, link_errors = check_links(site)
    errors = example_errors + link_errors
    for error in errors:
        print(error)
    print(f"Checked {examples} TOML examples and {links} local links; {len(errors)} errors.")
    return bool(errors)


if __name__ == "__main__":
    raise SystemExit(main())
