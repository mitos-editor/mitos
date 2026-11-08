#!/usr/bin/env python3
"""Build the coloured logo PNG from the two SVGs in Mitos.icon."""

from pathlib import Path
import subprocess
import xml.etree.ElementTree as ET


HERE = Path(__file__).resolve().parent
ASSETS = HERE.parents[1] / "Mitos.icon" / "Assets"
NS = "http://www.w3.org/2000/svg"
ET.register_namespace("", NS)


def main():
    background = ET.parse(ASSETS / "01-background-gradient.svg").getroot()
    logo = ET.parse(ASSETS / "02-logo-white.svg").getroot()
    defs = background.find(f"{{{NS}}}defs")
    rect = background.find(f"{{{NS}}}rect")
    path = logo.find(f"{{{NS}}}path")
    if defs is None or rect is None or path is None:
        raise ValueError("Expected background gradient definitions and a logo path")
    path.set("fill", rect.attrib["fill"])
    logo.insert(0, defs)
    ET.indent(logo, space="  ")
    subprocess.run([
        "rsvg-convert", "--output", str(ASSETS / "03-logo-gradient.png"),
    ], input=ET.tostring(logo), check=True)


if __name__ == "__main__":
    main()
