use super::*;

// Progression: A -> B -> C -> D
//              as we press `A-)`
const A: &str = indoc! {"
    #(a|)#
    #(b|)#
    #(c|)#
    #[d|]#
    #(e|)#"
};

const B: &str = indoc! {"
    #(e|)#
    #(a|)#
    #(b|)#
    #(c|)#
    #[d|]#"
};

const C: &str = indoc! {"
    #[d|]#
    #(e|)#
    #(a|)#
    #(b|)#
    #(c|)#"
};

const D: &str = indoc! {"
    #(c|)#
    #[d|]#
    #(e|)#
    #(a|)#
    #(b|)#"
};

#[tokio::test(flavor = "multi_thread")]
async fn rotation_bindings_forward_counts_in_both_directions() -> anyhow::Result<()> {
    test((A, "2<A-)>", C)).await?;
    test((D, "2<A-(>", B)).await?;
    Ok(())
}
