#!/usr/bin/env python3
"""Export Icon Composer layers from mitos-icon.svg. Requires rsvg-convert."""

from copy import deepcopy
from pathlib import Path
import subprocess
import xml.etree.ElementTree as ET


HERE = Path(__file__).resolve().parent
SOURCE = HERE.parents[1] / "mitos-icon.svg"
ASSETS = HERE.parents[1] / "Mitos.icon" / "Assets"
NS = "http://www.w3.org/2000/svg"
ET.register_namespace("", NS)


def svg():
    return ET.Element(f"{{{NS}}}svg", {
        "width": "1024", "height": "1024", "viewBox": "0 0 512 512",
    })


def export(root, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    ET.indent(root, space="  ")
    ET.ElementTree(root).write(destination, encoding="unicode")


def main():
    source = ET.parse(SOURCE).getroot()
    defs = source.find(f"{{{NS}}}defs")
    path = source.find(f"{{{NS}}}path")
    if defs is None or path is None:
        raise ValueError("Expected gradient definitions and one logo path")

    background = svg()
    background.append(deepcopy(defs))
    ET.SubElement(background, f"{{{NS}}}rect", {
        "width": "512", "height": "512", "fill": path.attrib["fill"],
    })

    white = svg()
    white_path = deepcopy(path)
    white_path.set("fill", "#f7faff")
    white.append(white_path)

    gradient = svg()
    gradient.append(deepcopy(defs))
    gradient.append(deepcopy(path))

    export(background, ASSETS / "01-background-gradient.svg")
    export(white, ASSETS / "02-logo-white.svg")
    ET.indent(gradient, space="  ")
    subprocess.run([
        "rsvg-convert", "--output", str(ASSETS / "03-logo-gradient.png"),
    ], input=ET.tostring(gradient), check=True)


if __name__ == "__main__":
    main()
