#!/usr/bin/env python3
"""Measure key-write to synchronized PTY output using the production editor."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import statistics
import struct
import termios
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("binary", type=Path)
parser.add_argument("fixture", type=Path)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--runtime", type=Path)
parser.add_argument("--mode", choices=["typing", "motion", "scroll"], default="typing")
parser.add_argument("--keys", type=int, default=160)
parser.add_argument("--interval-ms", type=float, default=25)
parser.add_argument("--symbol", help="Edit this Rust function name instead of the first function")
parser.add_argument("--lsp", action="store_true", help="Use the included mock; withhold highlight responses")
args = parser.parse_args()
if args.keys <= 0 or args.interval_ms <= 0:
    parser.error("keys and interval must be positive")
repo = Path(__file__).resolve().parents[2]
args.output.mkdir(parents=True, exist_ok=False)
work = args.output.resolve()
fixture = work / args.fixture.name
fixture.write_bytes(args.fixture.read_bytes())
config = work / "config.toml"
config.write_text('[editor.lsp]\nenable = ' + str(args.lsp).lower() + '\n[editor.workspace-trust]\nprompt = false\n[terminal.keyboard]\nkitty-protocol = "disabled"\n')
file_arg = str(fixture)
if args.mode == "typing":
    text = fixture.read_text()
    match = re.search(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn (\w+)", text)
    if args.symbol:
        match = re.search(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn (" + re.escape(args.symbol) + r")\b", text)
        if not match:
            parser.error("symbol was not found in the fixture")
    if match:
        line = text.count("\n", 0, match.start(1)) + 1
        column = match.start(1) - text.rfind("\n", 0, match.start(1))
        file_arg += f":{line}:{column}"
if args.lsp:
    directory = work / "config" / "mitos"
    directory.mkdir(parents=True)
    server = Path(__file__).with_name("server.py").resolve()
    (directory / "languages.toml").write_text('[[language]]\nname = "rust"\nlanguage-servers = ["benchmark"]\n[language-server.benchmark]\ncommand = "python3"\nargs = ' + json.dumps([str(server), str(work / "requests.jsonl")]) + '\n')
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
binary = str(args.binary.resolve())
pid = os.fork()
if pid == 0:
    os.close(master)
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    for fd in (0, 1, 2):
        os.dup2(slave, fd)
    if slave > 2:
        os.close(slave)
    env = os.environ.copy()
    env.update(TERM="xterm-256color", COLORTERM="truecolor", CARGO_MANIFEST_DIR=str(repo / "crates/term"), MITOS_RUNTIME=str((args.runtime or repo / "runtime").resolve()), XDG_CONFIG_HOME=str(work / "config"), XDG_CACHE_HOME=str(work / "cache"), XDG_DATA_HOME=str(work / "data"))
    env.pop("TERM_PROGRAM", None)
    env.pop("MITOS_PERF_TRACE", None)
    os.chdir(work)
    os.execve(binary, [binary, "--config", str(config), "--log", str(work / "editor.log"), file_arg], env)
os.close(slave)
os.set_blocking(master, False)
output = bytearray()
frames = 0
last_frame_ns = 0
boundary = b"\x1b[?2026l"
previous_tail = b""

def read(timeout):
    global frames, last_frame_ns, previous_tail
    ready, _, _ = select.select([master], [], [], max(0, timeout))
    if not ready:
        return
    try:
        data = os.read(master, 1 << 20)
    except (BlockingIOError, OSError):
        return
    if b"\x1b[c" in data or b"\x1b[0c" in data:
        os.write(master, b"\x1b[?2026;2$y\x1b[?1;2c")
    if b"\x1b[6n" in data:
        os.write(master, b"\x1b[1;1R")
    output.extend(data)
    combined = previous_tail + data
    count = combined.count(boundary)
    previous_tail = combined[-(len(boundary) - 1):]
    if count:
        frames += count
        last_frame_ns = time.monotonic_ns()

def drain(seconds):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        read(min(0.005, deadline - time.monotonic()))

def send(data, settle=0.2):
    os.write(master, data)
    drain(settle)

reaped = False
try:
    deadline = time.monotonic() + 15
    while not frames and time.monotonic() < deadline:
        read(0.05)
    if not frames:
        raise RuntimeError("No initial synchronized frame")
    drain(1)
    # Clear any pending input prefix. Check the emitted status rather than
    # assuming an initialization callback cannot overlap the first key.
    send(b"\x1b")
    if args.mode == "typing":
        for _ in range(3):
            mark = len(output)
            send(b"i")
            if b"INS" in output[mark:]:
                break
            send(b"\x1b")
        else:
            raise RuntimeError("Insert mode was not confirmed")
    drain(0.5)
    first_frame, first_byte = frames, len(output)
    samples, frames_per_key = [], []
    for index in range(args.keys):
        key = ([b"x", b"\x7f"][index % 2] if args.mode == "typing" else [b"l", b"h"][index % 2] if args.mode == "motion" else b"j" if index < args.keys // 2 else b"k")
        old_frames = frames
        start = time.monotonic_ns()
        os.write(master, key)
        deadline = time.monotonic() + 3
        while frames == old_frames and time.monotonic() < deadline:
            read(0.002)
        if frames == old_frames:
            raise RuntimeError(f"Key {index} produced no frame")
        samples.append((last_frame_ns - start) / 1e6)
        deadline = start / 1e9 + args.interval_ms / 1000
        while time.monotonic() < deadline:
            read(min(0.002, deadline - time.monotonic()))
        frames_per_key.append(frames - old_frames)
    steady_frames = frames - first_frame
    (work / "measured-terminal.bin").write_bytes(output[first_byte:])
    drain(1)
    settled_frames = frames - first_frame
    send(b"\x1b")
    send(b":qa!\r", 0.4)
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        result = os.waitpid(pid, os.WNOHANG)
        if result[0]:
            reaped = True
            break
        read(0.02)
    if not reaped or result[1]:
        raise RuntimeError("Editor did not exit successfully")
    ordered = sorted(samples)
    result = dict(fixture=str(args.fixture), bytes=fixture.stat().st_size, mode=args.mode, keys=args.keys, interval_ms=args.interval_ms, p50_ms=statistics.median(samples), p95_ms=ordered[int((len(ordered)-1)*0.95)], steady_frames=steady_frames, settled_frames=settled_frames, frames_per_key=frames_per_key, samples_ms=samples)
    (work / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k not in ("frames_per_key", "samples_ms")}))
finally:
    (work / "terminal.bin").write_bytes(output)
    if not reaped:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    os.close(master)
