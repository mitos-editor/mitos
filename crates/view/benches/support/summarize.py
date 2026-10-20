#!/usr/bin/env python3
"""Combine Criterion medians and allocation probes without dropping workloads."""

import argparse
import csv
import json
from pathlib import Path
import re
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("criterion_dir", type=Path)
parser.add_argument("allocation_dir", type=Path)
args = parser.parse_args()
tags = ["ropey1", "ropey2-migration", "ropey2-optimized"]
allocations = {}
for tag in tags:
    entries = {}
    for line in (args.allocation_dir / f"allocations-{tag}.log").read_text().splitlines():
        match = re.fullmatch(r"(ropey/.*): (\d+) allocations, (\d+) requested bytes", line)
        if match:
            entries[match[1]] = (int(match[2]), int(match[3]))
    allocations[tag] = entries

writer = csv.writer(sys.stdout, lineterminator="\n")
writer.writerow(["workload"] + [f"{tag}_{unit}" for tag in tags for unit in ["ns", "allocations", "bytes"]])
for workload in sorted(args.criterion_dir.glob("ropey_*")):
    name = json.loads((workload / tags[0] / "benchmark.json").read_text())["full_id"]
    row = [name]
    for tag in tags:
        estimate = json.loads((workload / tag / "estimates.json").read_text())
        row.extend([round(estimate["median"]["point_estimate"], 1), *allocations[tag][name]])
    writer.writerow(row)
