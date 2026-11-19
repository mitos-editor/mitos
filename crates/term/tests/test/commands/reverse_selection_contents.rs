use super::*;

const A: &str = indoc! {"
    #(a|)#
    #(b|)#
    #(c|)#
    #[d|]#
    #(e|)#"
};
const A_REV: &str = indoc! {"
    #(e|)#
    #[d|]#
    #(c|)#
    #(b|)#
    #(a|)#"
};
const CMD: &str = "<space>?reverse_selection_contents<ret>";

#[tokio::test(flavor = "multi_thread")]
async fn reverse_selection_contents() -> anyhow::Result<()> {
    test((A, CMD, A_REV)).await?;

    Ok(())
}
