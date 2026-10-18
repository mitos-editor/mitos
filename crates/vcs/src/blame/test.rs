#[cfg(feature = "git")]
mod git {
    use super::super::*;
    use crate::git::test::{create_commit_with_message, empty_git_repo};

    #[test]
    fn committed_history_tracks_insertions_deletions_and_replacements() {
        let repo = empty_git_repo();
        let file = repo.path().join("file.txt");
        // Fix line endings explicitly so this history also runs on Windows.
        std::fs::write(repo.path().join(".gitattributes"), "* text eol=lf\n").unwrap();
        let history: &[(&str, &[&str], &[&str])] = &[
            ("1", &["fn main() {", "", "}"], &["1", "1", "1"]),
            ("2", &["fn main() {", "  one", "}"], &["1", "2", "1"]),
            (
                "3",
                &["fn main() {", "  one", "  two", "}"],
                &["1", "2", "3", "1"],
            ),
            ("4", &["fn main() {", "  two", "}"], &["1", "3", "1"]),
            (
                "5",
                &[
                    "  six",
                    "  three",
                    "fn main() {",
                    "  five",
                    "  four",
                    "  two",
                    "  five",
                    "  four",
                    "}",
                    "  five",
                    "  four",
                ],
                &["5", "5", "1", "5", "5", "3", "5", "5", "1", "5", "5"],
            ),
            (
                "6",
                &["  six", "  three", "  four", "}", "  five", "  four"],
                &["5", "5", "5", "1", "5", "5"],
            ),
            (
                "7",
                &["  2", "  four", "  1", "}", "]", "  four"],
                &["7", "5", "7", "1", "7", "5"],
            ),
        ];
        for (title, lines, expected) in history {
            std::fs::write(&file, format!("{}\n", lines.join("\n"))).unwrap();
            create_commit_with_message(repo.path(), true, title);
            let blame = FileBlame::try_new(file.clone(), true).unwrap();
            for (line, expected) in expected.iter().enumerate() {
                assert_eq!(
                    blame
                        .blame_for_line(line as u32)
                        .unwrap()
                        .parse_format("{title}"),
                    *expected,
                    "commit {title}, line {line}"
                );
            }
            assert!(blame.blame_for_line(lines.len() as u32).is_none());
            // Working-tree edits do not alter the committed snapshot.
            std::fs::write(&file, "uncommitted\n").unwrap();
            let unchanged =
                FileBlame::try_refresh(file.clone(), true, Some(blame.clone())).unwrap();
            assert!(Arc::ptr_eq(&blame, &unchanged));
        }
    }

    #[test]
    fn linked_worktree_and_nested_paths_can_be_blamed_with_reduced_trust() {
        let repo = empty_git_repo();
        let nested = repo.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let file = nested.join("file.txt");
        std::fs::write(&file, "first\n").unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        crate::git::test::exec_git_cmd(
            &["worktree", "add", "-b", "linked", "worktree"],
            repo.path(),
        );
        let file = repo.path().join("worktree/nested/file.txt");
        // Windows canonical paths use a verbatim prefix unlike Git's metadata.
        for file in [file.clone(), file.canonicalize().unwrap()] {
            let blame = FileBlame::try_new(file, false).unwrap();
            assert_eq!(
                blame.blame_for_line(0).unwrap().parse_format("{title}"),
                "initial"
            );
        }
    }

    #[test]
    fn refresh_reuses_only_the_same_file_revision_and_trust() {
        let repo = empty_git_repo();
        let file = repo.path().join("file.txt");
        let other = repo.path().join("other.txt");
        std::fs::write(&file, "first\n").unwrap();
        std::fs::write(&other, "other\n").unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        let original = FileBlame::try_new(file.clone(), true).unwrap();
        let refreshed = FileBlame::try_refresh(file.clone(), true, Some(original.clone())).unwrap();
        assert!(Arc::ptr_eq(&original, &refreshed));
        let untrusted =
            FileBlame::try_refresh(file.clone(), false, Some(original.clone())).unwrap();
        assert!(!Arc::ptr_eq(&original, &untrusted));
        let different_file = FileBlame::try_refresh(other, true, Some(original.clone())).unwrap();
        assert!(!Arc::ptr_eq(&original, &different_file));
        // Even an unchanged blob must get fresh blame after a HEAD change.
        crate::git::test::exec_git_cmd(&["commit", "--allow-empty", "-m", "new HEAD"], repo.path());
        let new_head = FileBlame::try_refresh(file, true, Some(original.clone())).unwrap();
        assert!(!Arc::ptr_eq(&original, &new_head));
        assert_ne!(original.source.revision, new_head.source.revision);
    }

    #[test]
    fn ranges_share_commits_and_borrow_metadata() {
        let repo = empty_git_repo();
        let file = repo.path().join("file.txt");
        std::fs::write(&file, "first\nsecond\nthird\n").unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        std::fs::write(&file, "first\nchanged\nthird\n").unwrap();
        create_commit_with_message(repo.path(), true, "middle");
        let blame = FileBlame::try_new(file, true).unwrap();
        assert_eq!(blame.ranges.len(), 3);
        assert_eq!(blame.commits.len(), 2);
        assert!(std::ptr::eq(
            blame.blame_for_line(0).unwrap(),
            blame.blame_for_line(2).unwrap()
        ));
        assert_eq!(
            blame.blame_for_line(1).unwrap().parse_format("{title}"),
            "middle"
        );
        assert!(blame.blame_for_line(3).is_none());
    }

    #[test]
    fn single_commit_keeps_one_range_for_a_large_file() {
        let repo = empty_git_repo();
        let file = repo.path().join("file.txt");
        std::fs::write(&file, "line\n".repeat(10_000)).unwrap();
        create_commit_with_message(repo.path(), true, "initial");
        let blame = FileBlame::try_new(file, true).unwrap();
        assert_eq!(blame.ranges.len(), 1);
        assert_eq!(blame.ranges[0].lines, 0..10_000);
        assert_eq!(blame.commits.len(), 1);
        assert!(blame.blame_for_line(9_999).is_some());
        assert!(blame.blame_for_line(10_000).is_none());
    }
}

mod formatting {
    use super::super::LineBlame;

    #[test]
    fn format_borrows_metadata_with_a_large_unused_body() {
        let blame = LineBlame {
            commit_body: Some("x".repeat(1024 * 1024)),
            ..bob()
        };
        assert_eq!(blame.parse_format("{commit}"), "f14ab1cf");
        assert_eq!(blame.parse_format("{body}").len(), 1024 * 1024);
        assert_eq!(blame.parse_format("{time-ago} • {commit}"), "f14ab1cf");
        let dated = LineBlame {
            timestamp: Some(0),
            ..blame
        };
        let formatted = dated.parse_format("{time-ago} / {time-ago}");
        let (first, second) = formatted.split_once(" / ").unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn format_preserves_literals_unknown_variables_and_unicode() {
        assert_eq!(bob().parse_format("literal text"), "literal text");
        assert_eq!(
            bob().parse_format("by {author} today"),
            "by Bob TheBuilder today"
        );
        assert_eq!(
            bob().parse_format("{unknown} → {title}"),
            "{unknown} → feat!: extend house"
        );
        assert_eq!(bob().parse_format("{unknown"), "{unknown");
        assert_eq!(bob().parse_format("{未知} 🌱"), "{未知} 🌱");
    }

    fn bob() -> LineBlame {
        LineBlame {
            commit_id: "f14ab1cf00000000000000000000000000000000".into(),
            commit_hash: Some("f14ab1cf".to_owned()),
            author_name: Some("Bob TheBuilder".to_owned()),
            author_email: Some("bob@bob.com".to_owned()),
            commit_date: Some("2028-01-10".to_owned()),
            commit_title: Some("feat!: extend house".to_owned()),
            commit_body: Some("BREAKING CHANGE: Removed door".to_owned()),
            timestamp: None,
        }
    }

    #[test]
    fn inline_blame_format_parser() {
        let format = "{author}, {date} • {title} • {commit}";

        assert_eq!(
            bob().parse_format(format),
            "Bob TheBuilder, 2028-01-10 • feat!: extend house • f14ab1cf".to_owned()
        );
        assert_eq!(
            LineBlame {
                author_name: None,
                ..bob()
            }
            .parse_format(format),
            "2028-01-10 • feat!: extend house • f14ab1cf".to_owned()
        );
        assert_eq!(
            LineBlame {
                commit_date: None,
                ..bob()
            }
            .parse_format(format),
            "Bob TheBuilder • feat!: extend house • f14ab1cf".to_owned()
        );
        assert_eq!(
            LineBlame {
                commit_title: None,
                author_email: None,
                ..bob()
            }
            .parse_format(format),
            "Bob TheBuilder, 2028-01-10 • f14ab1cf".to_owned()
        );
        assert_eq!(
            LineBlame {
                commit_hash: None,
                ..bob()
            }
            .parse_format(format),
            "Bob TheBuilder, 2028-01-10 • feat!: extend house".to_owned()
        );
        assert_eq!(
            LineBlame {
                commit_date: None,
                author_name: None,
                ..bob()
            }
            .parse_format(format),
            "feat!: extend house • f14ab1cf".to_owned()
        );
        assert_eq!(
            LineBlame {
                author_name: None,
                commit_title: None,
                ..bob()
            }
            .parse_format(format),
            "2028-01-10 • f14ab1cf".to_owned()
        );
    }
}

#[cfg(not(feature = "git"))]
#[test]
fn blame_reports_when_git_support_is_disabled() {
    assert!(super::FileBlame::try_new("missing".into(), false)
        .unwrap_err()
        .to_string()
        .contains("not compiled in"));
}
