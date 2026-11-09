"""Check the grammar archive shared by all release platforms."""

from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


class GrammarBundleTests(unittest.TestCase):
    def test_bundle_contains_sources_without_links_or_git_metadata(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "runtime/grammars/sources"
            parser = source / "rpmspec/rpmspec/src/parser.c"
            parser.parent.mkdir(parents=True)
            parser.write_text("int parser_fixture;\n")
            header = parser.parent / "tree_sitter/parser.h"
            header.parent.mkdir()
            header.write_text("/* header fixture */\n")
            queries = source / "rpmspec/rpmspec/queries"
            queries.mkdir()
            (queries / "rpmspec").symlink_to(".", target_is_directory=True)
            binary = source / "rpmspec/neovim/parser/rpmspec.so"
            binary.parent.mkdir(parents=True)
            binary.symlink_to("../../build/rpmspec/libtree-sitter-rpmspec.so")
            metadata = source / "rpmspec/.git/config"
            metadata.parent.mkdir()
            metadata.write_text("fixture metadata must not be shipped\n")
            # Also cover submodule/worktree metadata stored as a file.
            (source / "rpmspec/rpmspec/.git").write_text("gitdir: fixture\n")
            subprocess.run(
                ["bash", str(ROOT / ".github/scripts/bundle-grammars.sh"),
                 str(source), str(root / "grammars.tar.xz")],
                check=True,
            )
            with tarfile.open(root / "grammars.tar.xz") as archive:
                members = archive.getmembers()
                self.assertFalse([m.name for m in members if m.issym() or m.islnk()])
                self.assertFalse([m.name for m in members if ".git" in Path(m.name).parts])
                self.assertEqual(archive.extractfile("./rpmspec/rpmspec/src/parser.c").read(), parser.read_bytes())
                self.assertEqual(archive.extractfile("./rpmspec/rpmspec/src/tree_sitter/parser.h").read(), header.read_bytes())


if __name__ == "__main__":
    unittest.main()
