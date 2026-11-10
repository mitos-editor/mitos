#!/usr/bin/env python3
"""Drive a production Mitos binary through a fixed-size POSIX pseudo-terminal."""
import argparse
import codecs
import errno
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import pty
import re
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time
import unicodedata

FRAME_START = b"\x1b[?2026h"
FRAME_END = b"\x1b[?2026l"


class Frames:
    """Count frame terminators even when reads split an escape sequence."""
    def __init__(self):
        self.tail = b""
        self.opened = False

    def feed(self, data):
        combined = self.tail + data
        count = offset = 0
        while offset < len(combined):
            start, end = combined.find(FRAME_START, offset), combined.find(FRAME_END, offset)
            positions = [p for p in [start, end] if p >= 0]
            if not positions:
                break
            position = min(positions)
            if position == start:
                self.opened = True
            elif self.opened:
                count += 1
                self.opened = False
            offset = position + len(FRAME_END)
        self.tail = combined[max(offset, len(combined) - len(FRAME_END) + 1):]
        return count


class Screen:
    """Small screen model for ASCII scenario markers, not a terminal renderer."""
    def __init__(self, width=120, height=40):
        self.width, self.height = width, height
        self.cells = [[" "] * width for _ in range(height)]
        self.row = self.column = 0
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def text(self):
        return "\n".join("".join(line) for line in self.cells)

    def feed(self, data):
        data = self.pending + self.decoder.decode(data)
        self.pending = ""
        i = 0
        while i < len(data):
            ch = data[i]
            if ch == "\x1b":
                if i + 1 == len(data):
                    break
                kind = data[i + 1]
                if kind in "]P_^":
                    end = re.search("\x07|\x1b\\\\", data[i + 2:])
                    if end is None:
                        break
                    i += 2 + end.end()
                    continue
                if kind == "[":
                    end = re.search(r"[@-~]", data[i + 2:])
                    if end is None:
                        break
                    final = i + 2 + end.start()
                    params = data[i + 2:final]
                    code = data[final]
                    if not params.startswith(("?", ">", "<", "=")):
                        values = [int(n) if n.isdecimal() else 0 for n in params.split(";")]
                        n = values[0] or 1
                        if code in "Hf":
                            self.row = min(n - 1, self.height - 1)
                            self.column = min(((values[1] or 1) if len(values) > 1 else 1) - 1, self.width - 1)
                        elif code == "G":
                            self.column = min(n - 1, self.width - 1)
                        elif code == "d":
                            self.row = min(n - 1, self.height - 1)
                        elif code in "ABCD":
                            self.row = max(0, min(self.height - 1, self.row + {"A": -n, "B": n}.get(code, 0)))
                            self.column = max(0, min(self.width - 1, self.column + {"C": n, "D": -n}.get(code, 0)))
                        elif code == "J" and values[0] in [2, 3]:
                            self.cells = [[" "] * self.width for _ in range(self.height)]
                        elif code == "K":
                            start = 0 if values[0] in [1, 2] else self.column
                            stop = self.column + 1 if values[0] == 1 else self.width
                            self.cells[self.row][start:stop] = [" "] * (stop - start)
                        elif code == "X":
                            stop = min(self.column + n, self.width)
                            self.cells[self.row][self.column:stop] = [" "] * (stop - self.column)
                    i = final + 1
                    continue
                i += 2
                continue
            if ch == "\r":
                self.column = 0
            elif ch == "\n":
                self.row = min(self.row + 1, self.height - 1)
            elif ch == "\b":
                self.column = max(0, self.column - 1)
            elif ch >= " " and not unicodedata.combining(ch):
                if self.column < self.width:
                    self.cells[self.row][self.column] = ch
                self.column += 2 if unicodedata.east_asian_width(ch) in "WF" else 1
            i += 1
        self.pending = data[i:]


def percentile(values, percent):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * percent / 100) - 1)]


def rss_kib(pid):
    try:
        result = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)],
                                capture_output=True, text=True, check=False)
    except OSError:
        return None
    return int(result.stdout.strip()) if result.returncode == 0 and result.stdout.strip() else None


class Session:
    def __init__(self, binary, runtime, root, document, config):
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        env = dict(os.environ, TERM="xterm-256color", COLORTERM="truecolor",
                   MITOS_RUNTIME=str(runtime), MITOS_CONFIG_DIR=str(config.parent),
                   XDG_CONFIG_HOME=str(config.parent / "xdg-config"),
                   XDG_DATA_HOME=str(config.parent / "xdg-data"),
                   XDG_STATE_HOME=str(config.parent / "xdg-state"),
                   XDG_CACHE_HOME=str(config.parent / "xdg-cache"),
                   MITOS_LOG_LEVEL="error", GIT_CONFIG_GLOBAL=os.devnull,
                   GIT_CONFIG_NOSYSTEM="1")
        # Do not inherit host terminal/multiplexer claims into the emulated PTY.
        for key in list(env):
            if key.startswith("GIT_") or key in ["TERM_PROGRAM", "TERM_PROGRAM_VERSION", "TMUX", "ZELLIJ", "KITTY_WINDOW_ID", "WEZTERM_PANE", "ITERM_SESSION_ID", "VTE_VERSION", "WEZTERM_EXECUTABLE", "KONSOLE_VERSION"]:
                env.pop(key, None)
        env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
        self.started = time.perf_counter_ns()
        try:
            self.process = subprocess.Popen(
                [str(binary), "--config", str(config), "--log", str(config.parent / "editor.log"),
                 str(document)], cwd=root, env=env, stdin=slave, stdout=slave, stderr=slave,
                start_new_session=True, preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
        except BaseException:
            os.close(master)
            raise
        finally:
            os.close(slave)
        self.master = master
        self.frames = Frames()
        self.screen = Screen()
        self.queries = b""

    def close(self):
        # Never let a failed workload leave an editor or its subprocesses behind.
        try:
            try:
                os.killpg(self.process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                pass
            # The group can outlive its leader. A final kill also removes any
            # descendants that ignored SIGTERM after the editor itself exited.
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.process.wait(timeout=3)
        finally:
            os.close(self.master)

    def read(self, timeout):
        if not select.select([self.master], [], [], timeout)[0]:
            return b""
        try:
            data = os.read(self.master, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                raise RuntimeError("editor closed the PTY") from error
            raise
        if not data:
            raise RuntimeError("editor closed the PTY")
        # Minimal terminal replies keep capability probes out of measured input.
        replies = {
            b"\x1b[6n": b"\x1b[1;1R",
            b"\x1b[16t": b"\x1b[6;16;8t",
            b"\x1b[5n": b"\x1b[0n",
            b"\x1b[>c": b"\x1b[>0;1;0c",
            b"\x1b[?u": b"\x1b[?0u",
            b"\x1b[?2026$p": b"\x1b[?2026;1$y",
            b"\x1b]11;?\x1b\\": b"\x1b]11;rgb:0000/0000/0000\x1b\\",
            b"\x1b[c": b"\x1b[?62;4;6;22c",
        }
        combined = self.queries + data
        responses = []
        for query, reply in replies.items():
            responses.extend((match.start(), reply) for match in re.finditer(re.escape(query), combined))
        for match in re.finditer(rb"\x1b_G([^\x1b]*)\x1b\\", combined):
            identifier = re.search(rb"(?:^|,)i=(\d+)", match.group(1))
            if identifier:
                responses.append((match.start(), b"\x1b_Gi=" + identifier[1] + b";ENOTSUP\x1b\\"))
        responses = b"".join(reply for _, reply in sorted(responses))
        if responses:
            os.write(self.master, responses)
        # Only incomplete queries need to survive the current read.
        self.queries = next((combined[-size:] for size in range(20, 0, -1)
                             if any(query.startswith(combined[-size:])
                                    and len(combined[-size:]) < len(query)
                                    for query in replies)), b"")
        self.screen.feed(data)
        return data

    def measure(self, name, payload=None, started=None, quiet=0.15, timeout=20, ready=None):
        if ready is None:
            raise ValueError("Every measured action needs an explicit visible-state predicate")
        start = time.perf_counter_ns() if started is None else started
        if payload is not None:
            os.write(self.master, payload)
        deadline = time.monotonic() + timeout
        last_frame = None
        first_frame = None
        ready_frame = None
        count = byte_count = 0
        while time.monotonic() < deadline:
            data = self.read(min(quiet, max(0, deadline - time.monotonic())))
            now = time.perf_counter_ns()
            byte_count += len(data)
            new_frames = self.frames.feed(data)
            if new_frames:
                count += new_frames
                first_frame = now if first_frame is None else first_frame
                last_frame = now
                if not self.frames.opened and ready(self.screen.text()):
                    ready_frame = now if ready_frame is None else ready_frame
                else:
                    ready_frame = None
            if self.frames.opened:
                ready_frame = None
            if ready_frame is not None and not self.screen.pending and (now - last_frame) / 1e9 >= quiet and not data:
                return {"workload": name, "first_frame_ns": first_frame - start,
                        "ready_frame_ns": ready_frame - start,
                        "last_frame_ns": last_frame - start, "frames": count,
                        "output_bytes": byte_count, "rss_kib": rss_kib(self.process.pid)}
            if self.process.poll() is not None:
                raise RuntimeError(f"editor exited during {name}: {self.process.returncode}")
        raise TimeoutError(f"{name}: no ready synchronized frame within {timeout}s; frames={count}, output_bytes={byte_count}\n{self.screen.text()}")

    def idle(self, duration=0.5):
        deadline = time.monotonic() + duration
        frames = byte_count = 0
        while time.monotonic() < deadline:
            data = self.read(max(0, deadline - time.monotonic()))
            byte_count += len(data)
            frames += self.frames.feed(data)
        return {"workload": "idle", "duration_ns": int(duration * 1e9),
                "frames": frames, "output_bytes": byte_count,
                "rss_kib": rss_kib(self.process.pid)}


def fixtures(root, files, commits):
    git_env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    git_env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
    (root / "ordinary.txt").write_text("ordinary source line\n" * 1000)
    (root / "large.txt").write_text("ordinary source line\n" * 100_000)
    (root / "long-line.txt").write_text("x" * 1_000_000 + "\nnext line\n")
    for i in range(files):
        directory = root / "src" / f"package-{i % 50:02}"
        directory.mkdir(parents=True, exist_ok=True)
        (directory / f"module-{i:06}.txt").write_text(f"preview {i}\n" * 100)
    subprocess.run(["git", "init", "--quiet", str(root)], check=True, env=git_env)
    # Fast-import builds real history without hundreds of worktree/commit subprocesses.
    records = []
    for i in range(commits):
        content = f"history {i}\n".encode()
        message = f"workload commit {i}\n".encode()
        records.append(b"commit refs/heads/workload\n" +
                       f"committer Workload <workload@example.invalid> {1700000000 + i} +0000\n".encode() +
                       f"data {len(message)}\n".encode() + message +
                       f"M 100644 inline history.txt\ndata {len(content)}\n".encode() + content + b"\n")
    subprocess.run(["git", "-C", str(root), "fast-import", "--quiet"],
                   input=b"".join(records), check=True, env=git_env)
    subprocess.run(["git", "-C", str(root), "symbolic-ref", "HEAD", "refs/heads/workload"], check=True, env=git_env)
    subprocess.run(["git", "-C", str(root), "reset", "--quiet", "--hard", "HEAD"], check=True, env=git_env)


def visible(*markers, absent=()):
    return lambda screen: all(marker in screen for marker in markers) and all(marker not in screen for marker in absent)


def picker_ready(*markers):
    return lambda screen: visible(*markers, absent=("(running)", "/?"))(screen) and re.search(r"\d+/\d+", screen) is not None


def preview_ids(screen):
    return tuple(re.findall(r"preview (\d+)", screen))


def binary_hash(binary):
    with binary.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def run(args):
    binary, runtime = args.binary.resolve(), args.runtime.resolve()
    results = []
    with tempfile.TemporaryDirectory(prefix="mitos-workloads-") as temporary:
        base = Path(temporary)
        root = base / "workspace"
        root.mkdir()
        fixtures(root, args.files, args.commits)
        config = base / "config" / "config.toml"
        config.parent.mkdir()
        config.write_text('''[editor.completion]
auto-trigger = false
[editor.lsp]
enable = false
[editor.files.watch]
enable = false
[editor.files.reload.poll]
enable = false
[keys.normal]
F5 = "focus_sidebar"
F6 = "vcs_history_picker"
F7 = "file_picker"
''')
        for sample in range(args.samples):
            for document in ["ordinary.txt", "large.txt", "long-line.txt"]:
                session = Session(binary, runtime, root, root / document, config)
                try:
                    rows = [session.measure(f"startup/{document}", started=session.started, ready=visible(document)),
                            session.measure(f"edit/{document}", b"iZ\x1b", ready=visible("Zx" if document == "long-line.txt" else "Zordinary")), session.idle()]
                    for row in rows:
                        row.update(sample=sample, document=document)
                    results.extend(rows)
                finally:
                    session.close()
            session = Session(binary, runtime, root, root / "ordinary.txt", config)
            try:
                session.measure("workspace/startup", started=session.started, ready=visible("ordinary.txt"))
                rows = [session.measure("picker/open", b"\x1b[18~", ready=picker_ready("history.txt")),
                        session.measure("picker/filter", b"module-", ready=picker_ready("module-", "preview "))]
                for i in range(min(args.previews, args.files - 1)):
                    previous = preview_ids(session.screen.text())
                    rows.append(session.measure("picker/next-preview", b"\x0e",
                        ready=lambda screen: bool(preview_ids(screen)) and preview_ids(screen) != previous))
                rows.append(session.measure("picker/close", b"\x1b", ready=visible("ordinary.txt", absent=("module-",))))
                rows.append(session.measure("sidebar/open", b"\x1b[15~", ready=picker_ready("File Explorer")))
                rows.append(session.measure("sidebar/filter", b"/src/module", ready=picker_ready("File Explorer (filter)", "module-")))
                rows.append(session.measure("sidebar/query-edit", b"1", ready=picker_ready("src/module1")))
                rows.append(session.measure("sidebar/leave", b"\x05", ready=visible("File Explorer", absent=("(filter)",))))
                rows.append(session.measure("history/open", b"\x1b[17~", ready=visible("Git history", "workload commit")))
                rows.append(session.measure("history/filter", b"workload", ready=picker_ready("Git history", "Complete history")))
                rows.append(session.measure("history/close", b"\x1b", ready=visible("ordinary.txt", absent=("Git history",))))
                rows.append(session.idle())
                for row in rows:
                    row.update(sample=sample, document="workspace")
                results.extend(rows)
            finally:
                session.close()
    groups = {}
    for row in results:
        if "first_frame_ns" in row:
            groups.setdefault(row["workload"], []).append(row)
    version = subprocess.run([str(binary), "--version"], check=True, capture_output=True, text=True).stdout.strip()
    return {"schema_version": 1, "platform": platform.platform(), "binary": str(binary),
            "binary_sha256": binary_hash(binary),
            "version": version, "runtime": str(runtime), "terminal": [120, 40],
            "fixtures": {"files": args.files, "commits": args.commits,
                         "previews_requested": args.previews, "previews": min(args.previews, args.files - 1)},
            "measurements": results, "summary": {name: {"samples": len(values),
                **{metric: {f"p{p}_ns": percentile([row[metric] for row in values], p) for p in [50, 95, 99]}
                   for metric in ["first_frame_ns", "ready_frame_ns"]}}
                for name, values in groups.items()}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    for name, default in [("samples", 5), ("files", 10_000), ("commits", 1000), ("previews", 20)]:
        parser.add_argument(f"--{name}", type=int, default=default)
    args = parser.parse_args()
    if any(getattr(args, name) < 1 for name in ["samples", "files", "commits", "previews"]):
        parser.error("workload sizes and sample count must be positive")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(run(args), indent=2) + "\n")


if __name__ == "__main__":
    main()
