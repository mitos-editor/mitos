use anyhow::Context as _;
use std::time::Duration;
use view::{current_ref, editor::Action};

use super::Fixture;

impl Fixture {
    pub(super) fn words(&self) -> Vec<String> {
        let mut words = self.editor.handlers.word_index().matches("");
        words.sort();
        words
    }

    pub(super) async fn expect_words(&self, words: &[&str]) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.words() != words {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .with_context(|| format!("expected {words:?}, got {:?}", self.words()))
    }

    fn word_completion(&mut self, enabled: bool) {
        self.configure(|config| config.word_completion.enable = enabled);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn multiple_editors_index_only_their_own_documents_and_edits() -> anyhow::Result<()> {
    let mut first = Fixture::new("firstword\n")?;
    let mut second = Fixture::new("secondword\n")?;
    assert_eq!(
        current_ref!(first.editor).1.id(),
        current_ref!(second.editor).1.id()
    );
    first.expect_words(&["firstword"]).await?;
    second.expect_words(&["secondword"]).await?;
    first.replace("firstchanged\n");
    second.replace("secondchanged\n");
    first.expect_words(&["firstchanged"]).await?;
    second.expect_words(&["secondchanged"]).await?;
    // Closing must remove the word completely: duplicate hook registration leaks counts.
    first.close_current()?;
    first.expect_words(&[]).await?;
    second.expect_words(&["secondchanged"]).await?;
    drop(first);
    let mut replacement = Fixture::new("replacementword\n")?;
    replacement.expect_words(&["replacementword"]).await?;
    second.close_current()?;
    second.expect_words(&[]).await?;
    replacement.close_current()?;
    replacement.expect_words(&[]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn configuration_reset_discards_debounced_edits_before_reindexing() -> anyhow::Result<()> {
    let mut f = Fixture::new("seedword\n")?;
    f.expect_words(&["seedword"]).await?;
    f.replace("pendingword\n");
    f.word_completion(false);
    f.expect_words(&[]).await?;
    // The index must remain empty after the old edit's one-second debounce deadline.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(f.words().is_empty());
    f.word_completion(true);
    f.expect_words(&["pendingword"]).await?;
    f.replace("finalword\n");
    f.close_current()?;
    f.expect_words(&[]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn language_override_survives_global_word_completion_changes() -> anyhow::Result<()> {
    let mut f = Fixture::with_languages(
        "languageword\n",
        r#"
        [[language]]
        name = "words"
        scope = "source.words"
        file-types = ["words"]
        roots = []
        word-completion = { enable = true }
    "#,
    )?;
    let language_doc = current_ref!(f.editor).1.id();
    let other = f.dir.path().join("other.txt");
    std::fs::write(&other, "globalword\n")?;
    f.editor.open(&other, Action::Replace)?;
    f.expect_words(&["globalword", "languageword"]).await?;
    f.word_completion(false);
    f.expect_words(&["languageword"]).await?;
    f.word_completion(true);
    f.expect_words(&["globalword", "languageword"]).await?;
    f.close_current()?;
    f.expect_words(&["languageword"]).await?;
    assert!(f.editor.close_document(language_doc, true).is_ok());
    f.expect_words(&[]).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn batched_configuration_rebuild_keeps_all_documents() -> anyhow::Result<()> {
    let mut f = Fixture::new("seedword\n")?;
    // More documents than the old debounce channel's capacity, created without yielding.
    let mut expected = vec!["seedword".to_owned()];
    for i in 0..140 {
        f.editor.new_file(Action::Replace);
        let word = format!("batchword{i:03}");
        f.replace(&format!("{word}\n"));
        expected.push(word);
    }
    f.word_completion(false);
    f.word_completion(true);
    expected.sort();
    let expected: Vec<_> = expected.iter().map(String::as_str).collect();
    f.expect_words(&expected).await?;
    let documents: Vec<_> = f.editor.documents().map(|doc| doc.id()).collect();
    f.editor.new_file(Action::Replace);
    for document in documents {
        assert!(f.editor.close_document(document, true).is_ok());
    }
    f.expect_words(&[]).await?;
    Ok(())
}
