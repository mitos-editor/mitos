import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("build_site", ROOT / "book/build-site.py")
build_site = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build_site)


def response(releases, link=""):
    result = io.BytesIO(json.dumps(releases).encode())
    result.headers = {"Link": link}
    return result


class DocumentationTests(unittest.TestCase):
    def test_published_stable_releases_sorted_numerically(self):
        def release(tag):
            return dict(tag_name=tag, draft=False, prerelease=False)

        next_page = "https://api.github.com/repos/mitos-editor/mitos/releases?page=2"
        pages = [response([release("v0.9.0"), release("v0.10.0"), release("25.07")],
                          f'<{next_page}>; rel="next"'), response([
            dict(tag_name="v1.0.0", draft=True, prerelease=False),
            dict(tag_name="v0.11.0", draft=False, prerelease=True),
            release("v0.10.0"),
        ])]
        with patch.object(build_site, "urlopen", side_effect=pages) as request:
            self.assertEqual(build_site.release_versions(), ["v0.10.0", "v0.9.0"])
            self.assertEqual(request.call_count, 2)
            self.assertEqual(request.call_args.args[0].full_url, next_page)
        with patch.object(build_site, "urlopen", return_value=response([])):
            with self.assertRaisesRegex(ValueError, "No published stable releases"):
                build_site.release_versions()

    @unittest.skipUnless(shutil.which("mdbook"), "mdbook is required")
    def test_shallow_checkout_snapshot_content_navigation_and_manifest(self):
        with tempfile.TemporaryDirectory(prefix="mitos-docs-test-") as tmp:
            repo = Path(tmp)
            book = repo / "book"
            shutil.copytree(ROOT / "book", book, ignore=shutil.ignore_patterns(
                "site", "book", ".bin", "src", "__pycache__"))
            source = book / "src"
            source.mkdir()
            (source / "SUMMARY.md").write_text("# Summary\n\n- [Introduction](index.md)\n")
            (source / "index.md").write_text("# Introduction\n\nOriginal release content.\n")

            def git(*args):
                subprocess.run(["git", "-c", "core.fsmonitor=false", "-c", "user.name=Docs Test",
                                "-c", "user.email=docs@example.invalid", *args], cwd=repo,
                               check=True, capture_output=True)

            git("init")
            git("add", ".")
            git("commit", "-m", "initial")
            git("tag", "v0.1.0")
            (source / "index.md").write_text("# Introduction\n\nLatest release content.\n")
            git("add", ".")
            git("commit", "-m", "release")
            git("tag", "v0.2.0")
            git("tag", "v99.0.0")  # Unpublished tags must not become documentation versions.
            (source / "index.md").write_text("# Introduction\n\nDevelopment content.\n")
            (source / "SUMMARY.md").write_text(
                "# Summary\n\n- [Introduction](index.md)\n- [New feature](new-feature.md)\n")
            (source / "new-feature.md").write_text("# New feature\n")
            git("add", ".")
            git("commit", "-m", "development")
            checkout = repo / "checkout"
            subprocess.run(["git", "-c", "core.fsmonitor=false", "clone", "--depth=1",
                            "--no-tags", repo.as_uri(), str(checkout)], check=True,
                           capture_output=True)
            book = checkout / "book"
            releases = [dict(tag_name=tag, draft=False, prerelease=False)
                        for tag in ["v0.1.0", "v0.2.0"]]
            with (patch.object(build_site, "BOOK", book),
                  patch.object(build_site, "REPO", checkout),
                  patch.object(build_site, "urlopen", return_value=response(releases)),
                  patch.dict(os.environ, MDBOOK_OUTPUT__LINKCHECK2__OPTIONAL="true")):
                build_site.main()
            docs = book / "site/docs"
            self.assertIn("Latest release content.", (docs / "index.html").read_text())
            self.assertIn("Original release content.", (docs / "v0.1.0/index.html").read_text())
            main = (docs / "main/index.html").read_text()
            self.assertIn("Development content.", main)
            self.assertIn('<option value="main" selected>main (development)</option>', main)
            self.assertIn('class="header-brand" href="/"', main)
            release = (docs / "v0.2.0/index.html").read_text()
            self.assertIn('/blob/v0.2.0/book/src/index.md', release)
            self.assertEqual((docs / "index.html").read_bytes(), (docs / "v0.2.0/index.html").read_bytes())
            versions = json.loads((docs / "versions.json").read_text())
            self.assertEqual(set(versions), {"main", "v0.1.0", "v0.2.0"})
            self.assertIn("new-feature.html", versions["main"])
            self.assertNotIn("new-feature.html", versions["v0.2.0"])
            self.assertTrue((book / "site/install.sh").is_file())


if __name__ == "__main__":
    unittest.main()
