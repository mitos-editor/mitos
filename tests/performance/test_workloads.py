"""Deterministic checks for the PTY workload recorder; no timing assertions."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("workloads", Path(__file__).with_name("workloads.py"))
workloads = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(workloads)


class FrameTests(unittest.TestCase):
    def test_frame_markers_survive_every_read_boundary(self):
        stream = workloads.FRAME_START + b"rendered text" + workloads.FRAME_END
        for boundary in range(len(stream) + 1):
            with self.subTest(boundary=boundary):
                frames = workloads.Frames()
                self.assertEqual(frames.feed(stream[:boundary]) + frames.feed(stream[boundary:]), 1)
        frames = workloads.Frames()
        self.assertEqual(sum(frames.feed(bytes([byte])) for byte in stream), 1)

    def test_orphan_resets_and_open_frames_do_not_count_as_completed_frames(self):
        frames = workloads.Frames()
        self.assertEqual(frames.feed(workloads.FRAME_END), 0)
        self.assertEqual(frames.feed(workloads.FRAME_START + b"incomplete"), 0)
        self.assertEqual(frames.feed(workloads.FRAME_START + workloads.FRAME_END), 1)
        self.assertEqual(frames.feed(workloads.FRAME_END), 0)

    def test_multiple_frames_and_intervening_output_count_once_each(self):
        frame = workloads.FRAME_START + b"paint" + workloads.FRAME_END
        frames = workloads.Frames()
        self.assertEqual(frames.feed(b"probe" + frame + b"cursor" + frame), 2)
        self.assertEqual(frames.feed(b"unframed output"), 0)


class ScreenTests(unittest.TestCase):
    def test_fragmented_cursor_style_utf8_and_osc_sequences_reconstruct_markers(self):
        stream = (b"obsolete" + b"\x1b[2J\x1b[2;3H" + "界".encode() +
                  b"File \x1b[31mpicker\x1b[m\x1b]8;;https://example.invalid\x1b\\" +
                  b" ready\x1b]8;;\x1b\\")
        expected = workloads.Screen(width=40, height=4)
        expected.feed(stream)
        self.assertIn("File picker ready", expected.text())
        self.assertNotIn("obsolete", expected.text())
        self.assertNotIn("example.invalid", expected.text())
        for boundary in range(len(stream) + 1):
            with self.subTest(boundary=boundary):
                screen = workloads.Screen(width=40, height=4)
                screen.feed(stream[:boundary])
                screen.feed(stream[boundary:])
                self.assertEqual(screen.text(), expected.text())
        screen = workloads.Screen(width=40, height=4)
        for byte in stream:
            screen.feed(bytes([byte]))
        self.assertEqual(screen.text(), expected.text())

    def test_erased_titles_cannot_satisfy_the_next_scenario_marker(self):
        screen = workloads.Screen(width=40, height=4)
        screen.feed(b"Git history")
        self.assertIn("Git history", screen.text())
        screen.feed(b"\x1b[1;1H\x1b[2KFile picker")
        self.assertIn("File picker", screen.text())
        self.assertNotIn("Git history", screen.text())
        screen.feed(b"\x1b[2J")
        self.assertNotIn("File picker", screen.text())


class TerminalProbeTests(unittest.TestCase):
    def test_fragmented_status_query_replies_once_before_user_input(self):
        # The image capability reader waits for DSR. Omitting its reply leaves
        # a blocked reader that can consume the first editor key after timeout.
        session = workloads.Session.__new__(workloads.Session)
        session.master = 99
        session.queries = b""
        session.screen = workloads.Screen()
        with mock.patch.object(workloads.select, "select", return_value=([99], [], [])), \
             mock.patch.object(workloads.os, "read", side_effect=[b"\x1b[5", b"n", b"ordinary output"]), \
             mock.patch.object(workloads.os, "write") as write:
            session.read(0)
            write.assert_not_called()
            session.read(0)
            write.assert_called_once_with(99, b"\x1b[0n")
            session.read(0)
            write.assert_called_once_with(99, b"\x1b[0n")

    def test_cell_size_reply_precedes_status_reply_for_one_probe_batch(self):
        session = workloads.Session.__new__(workloads.Session)
        session.master = 99
        session.queries = b""
        session.screen = workloads.Screen()
        with mock.patch.object(workloads.select, "select", return_value=([99], [], [])), \
             mock.patch.object(workloads.os, "read", return_value=b"\x1b[16t\x1b[5n"), \
             mock.patch.object(workloads.os, "write") as write:
            session.read(0)
            write.assert_called_once_with(99, b"\x1b[6;16;8t\x1b[0n")


class ReadinessTests(unittest.TestCase):
    def test_loading_or_unsupported_history_is_not_a_ready_picker(self):
        ready = workloads.picker_ready("Git history", "Complete history")
        self.assertFalse(ready("No Git support included in this build"))
        self.assertFalse(ready("Git history Complete history (running) 10/10"))
        self.assertFalse(ready("Git history Complete history 10/?"))
        self.assertTrue(ready("Git history Complete history 10/10"))

    def test_every_action_requires_a_visible_state_predicate(self):
        session = workloads.Session.__new__(workloads.Session)
        with self.assertRaises(ValueError):
            session.measure("unspecified")

    def test_an_unterminated_ready_frame_cannot_finish_the_measurement(self):
        session = workloads.Session.__new__(workloads.Session)
        session.frames = workloads.Frames()
        session.screen = workloads.Screen()
        session.process = mock.Mock()
        session.process.poll.return_value = None
        chunks = iter([workloads.FRAME_START + b"loading" + workloads.FRAME_END +
                       workloads.FRAME_START + b"\x1b[2Jready"])

        def read(_timeout):
            data = next(chunks, b"")
            session.screen.feed(data)
            return data

        session.read = read
        with mock.patch.object(workloads, "rss_kib", return_value=None):
            with self.assertRaises(TimeoutError):
                session.measure("unterminated", ready=lambda screen: "ready" in screen,
                                quiet=0.001, timeout=0.01)


    def test_ready_timestamp_waits_for_the_closing_frame_marker(self):
        session = workloads.Session.__new__(workloads.Session)
        session.frames = workloads.Frames()
        session.screen = workloads.Screen()
        session.process = mock.Mock()
        session.process.poll.return_value = None
        chunks = iter([workloads.FRAME_START + b"loading" + workloads.FRAME_END +
                       workloads.FRAME_START + b"\x1b[2Jready", b"", workloads.FRAME_END])

        def read(_timeout):
            data = next(chunks, b"")
            session.screen.feed(data)
            return data

        session.read = read
        with mock.patch.object(workloads, "rss_kib", return_value=None):
            result = session.measure("closed", ready=lambda screen: "ready" in screen,
                                     quiet=0.001, timeout=0.1)
        self.assertEqual(result["frames"], 2)
        self.assertGreater(result["ready_frame_ns"], result["first_frame_ns"])


class StatisticTests(unittest.TestCase):
    def test_percentiles_use_nearest_rank_on_unsorted_samples(self):
        values = [9, 2, 4, 1, 5]
        self.assertEqual([workloads.percentile(values, p) for p in (50, 95, 99)], [4, 9, 9])
        self.assertEqual(values, [9, 2, 4, 1, 5])
        self.assertEqual(workloads.percentile([17], 99), 17)

    def test_memory_measurement_is_optional_when_ps_is_unavailable(self):
        with mock.patch.object(workloads.subprocess, "run", side_effect=FileNotFoundError):
            self.assertIsNone(workloads.rss_kib(123))
        with mock.patch.object(workloads.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "")):
            self.assertIsNone(workloads.rss_kib(123))


class FixtureTests(unittest.TestCase):
    def test_inherited_git_repository_variables_cannot_redirect_fixture_resets(self):
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            sentinel = base / "unrelated"
            target = base / "fixture"
            sentinel.mkdir()
            target.mkdir()
            subprocess.run(["git", "init", "--quiet", str(sentinel)], env=environment, check=True)
            tracked = sentinel / "preserve.txt"
            tracked.write_text("committed\n")
            subprocess.run(["git", "-C", str(sentinel), "add", "preserve.txt"], env=environment, check=True)
            subprocess.run(["git", "-C", str(sentinel), "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                            "commit", "--quiet", "-m", "initial"], env=environment, check=True)
            tracked.write_text("unrelated dirty work\n")
            hostile = dict(environment, GIT_DIR=str(sentinel / ".git"),
                           GIT_WORK_TREE=str(sentinel), GIT_INDEX_FILE=str(sentinel / ".git/index"))
            with mock.patch.dict(os.environ, hostile, clear=True):
                workloads.fixtures(target, files=1, commits=2)
            self.assertEqual(tracked.read_text(), "unrelated dirty work\n")
            count = subprocess.check_output(["git", "-C", str(target), "rev-list", "--count", "HEAD"],
                                            text=True, env=environment).strip()
            self.assertEqual(count, "2")

    def test_history_is_a_real_linear_repository_and_source_fixtures_survive_reset(self):
        # Prevent the caller's Git environment from changing this fixture test.
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
        with tempfile.TemporaryDirectory() as temporary, mock.patch.dict(os.environ, environment, clear=True):
            root = Path(temporary)
            workloads.fixtures(root, files=3, commits=4)

            def git(*args):
                return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()

            self.assertEqual(git("rev-list", "--count", "HEAD"), "4")
            self.assertEqual(git("symbolic-ref", "--short", "HEAD"), "workload")
            self.assertEqual(git("log", "--format=%s").splitlines(), [f"workload commit {i}" for i in reversed(range(4))])
            self.assertEqual(git("show", "HEAD:history.txt"), "history 3")
            self.assertEqual((root / "history.txt").read_text(), "history 3\n")
            self.assertEqual(len(list((root / "src").rglob("module-*.txt"))), 3)
            self.assertEqual((root / "ordinary.txt").read_text().count("\n"), 1_000)
            self.assertEqual((root / "large.txt").read_text().count("\n"), 100_000)
            self.assertEqual((root / "long-line.txt").read_text().splitlines()[0], "x" * 1_000_000)
            self.assertEqual(git("diff", "HEAD", "--stat"), "")


if __name__ == "__main__":
    unittest.main()
