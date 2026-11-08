#!/usr/bin/env python3
"""Render Mitos.icon and derive website and package icons from its appearances."""

import argparse
import base64
from copy import deepcopy
import json
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]
ICONS = ROOT / "assets" / "icons"
THEME = ROOT / "book" / "theme"


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


def resize(source, size, target):
    run("magick", source, "-colorspace", "sRGB", "-resize", f"{size}x{size}",
        "-depth", "8", "+profile", "exif", "-define",
        "png:exclude-chunk=date,time", target)


def site_document(source, destination):
    """Derive a softly tinted white canvas with the finished coloured logo."""
    document = json.loads((source / "icon.json").read_text())
    groups = [group for group in document["groups"]
              if any(layer.get("name") == "03-logo-gradient"
                     for layer in group.get("layers", []))]
    if len(groups) != 1:
        raise ValueError("Expected one group containing 03-logo-gradient")
    group = deepcopy(groups[0])
    for item in [group, *group["layers"]]:
        item.pop("opacity-specializations", None)
        item["opacity"] = 1
    backgrounds = [background for background in document["groups"]
                   if any(layer.get("name") == "01-background-gradient"
                          for layer in background.get("layers", []))]
    if len(backgrounds) != 1:
        raise ValueError("Expected one group containing 01-background-gradient")
    background = deepcopy(backgrounds[0])
    background.pop("opacity-specializations", None)
    background["opacity"] = 0.07
    background["name"] = "Subtle gradient background"
    document["groups"] = [group, background]
    document["fill"] = {"solid": "display-p3:1.00000,1.00000,1.00000,1.00000"}
    document.pop("fill-specializations", None)
    destination.mkdir(exist_ok=True)
    shutil.copytree(source / "Assets", destination / "Assets", dirs_exist_ok=True)
    (destination / "icon.json").write_text(json.dumps(document, indent=2) + "\n")
    return destination


def render(args, source, rendition, destination):
    run(args.ictool, source.resolve(), "--export-image", "--output-file", destination,
        "--platform", "macOS", "--rendition", rendition, "--width", "1024",
        "--height", "1024", "--scale", "1", "--design-generation", "27")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=ROOT / "Mitos.icon")
    parser.add_argument("--ictool", type=Path, default=Path(
        "/Applications/Icon Composer.app/Contents/Executables/ictool"))
    parser.add_argument("--skip-render", action="store_true",
                        help="Derive icons from the existing 1024px PNG exports")
    args = parser.parse_args()
    ICONS.mkdir(parents=True, exist_ok=True)
    site_master = ICONS / "mitos-site-light.png"

    with tempfile.TemporaryDirectory(prefix="mitos-icons-") as temporary:
        temporary = Path(temporary)
        if not args.skip_render:
            site_source = site_document(args.source, temporary / "Mitos-site.icon")
            render(args, site_source, "Default", site_master)
        favicon_data = {}
        for appearance, rendition in [("light", "Default"), ("dark", "Dark")]:
            master = ICONS / f"mitos-{appearance}.png"
            if not args.skip_render:
                render(args, args.source, rendition, master)

            run("magick", master, "-colorspace", "sRGB", "-define",
                "icon:auto-resize=256,128,64,48,32,16", ICONS / f"mitos-{appearance}.ico")

            iconset = temporary / f"mitos-{appearance}.iconset"
            iconset.mkdir()
            for size in [16, 32, 128, 256, 512]:
                resize(master, size, iconset / f"icon_{size}x{size}.png")
                resize(master, size * 2, iconset / f"icon_{size}x{size}@2x.png")
            run("iconutil", "--convert", "icns", "--output",
                ICONS / f"mitos-{appearance}.icns", iconset)

            favicon = temporary / f"favicon-{appearance}.png"
            resize(site_master if appearance == "light" else master, 64, favicon)
            favicon_data[appearance] = base64.b64encode(favicon.read_bytes()).decode("ascii")
            if appearance == "light":
                (THEME / "favicon.png").write_bytes(favicon.read_bytes())

    (THEME / "favicon.svg").write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" '
        'viewBox="0 0 64 64">\n'
        '  <style>\n'
        '    .dark { display: none; }\n'
        '    @media (prefers-color-scheme: dark) {\n'
        '      .light { display: none; }\n'
        '      .dark { display: inline; }\n'
        '    }\n'
        '  </style>\n'
        + ''.join(
            f'  <image class="{appearance}" width="64" height="64" '
            f'href="data:image/png;base64,{data}"/>\n'
            for appearance, data in favicon_data.items()
        ) + '</svg>\n', encoding="utf-8")


if __name__ == "__main__":
    main()
