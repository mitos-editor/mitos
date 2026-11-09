"""Exercise the upload guard with draft, published, and missing releases."""

import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


class DraftReleaseTests(unittest.TestCase):
    def lookup(self, release, exit_code=0):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        step = workflow.split("      - name: Find draft release\n", 1)[1].split("\n      - name:", 1)[0]
        script = step.split("        run: |\n", 1)[1]
        script = "\n".join(line[10:] for line in script.splitlines())
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            gh = root / "gh"
            gh.write_text("#!/bin/sh\nprintf '%s\\n' \"$FIXTURE_RELEASE\"\nexit \"$FIXTURE_EXIT\"\n")
            gh.chmod(0o755)
            output = root / "output"
            env = dict(os.environ, PATH=str(root) + os.pathsep + os.environ["PATH"],
                       FIXTURE_RELEASE=json.dumps(release), FIXTURE_EXIT=str(exit_code),
                       GITHUB_OUTPUT=str(output), GITHUB_REF_NAME="v0.2.1",
                       GITHUB_REPOSITORY="mitos-editor/mitos")
            result = subprocess.run(["bash", "-euo", "pipefail", "-c", script], env=env, capture_output=True)
            return result.returncode, output.read_text() if output.exists() else ""

    def test_draft_resolves_existing_release_id(self):
        self.assertEqual(self.lookup({"databaseId": 123, "isDraft": True}), (0, "release_id=123\n"))

    def test_published_release_refuses_upload(self):
        code, output = self.lookup({"databaseId": 123, "isDraft": False})
        self.assertNotEqual(code, 0)
        self.assertEqual(output, "")

    def test_missing_release_refuses_upload(self):
        code, output = self.lookup({}, exit_code=1)
        self.assertNotEqual(code, 0)
        self.assertEqual(output, "")


if __name__ == "__main__":
    unittest.main()
