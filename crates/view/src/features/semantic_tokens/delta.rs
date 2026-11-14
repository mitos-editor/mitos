//! Reconstruct protocol token arrays before interpreting their five-integer groups.

use lsp_client::lsp;

pub(super) fn reconstruct(
    previous: &[lsp::SemanticToken],
    response: lsp::SemanticTokensFullDeltaResult,
) -> Option<lsp::SemanticTokens> {
    match response {
        lsp::SemanticTokensFullDeltaResult::Tokens(tokens) => Some(tokens),
        lsp::SemanticTokensFullDeltaResult::TokensDelta(delta) => apply(previous, delta),
        lsp::SemanticTokensFullDeltaResult::PartialTokensDelta { edits } => apply(
            previous,
            lsp::SemanticTokensDelta {
                result_id: None,
                edits,
            },
        ),
    }
}

/// All edit offsets refer to the same previous integer array, and edits need
/// not arrive sorted. Reject the entire update when its bounds or shape are invalid.
fn apply(
    previous: &[lsp::SemanticToken],
    mut delta: lsp::SemanticTokensDelta,
) -> Option<lsp::SemanticTokens> {
    let previous_len = previous.len().checked_mul(5)?;
    delta.edits.sort_by_key(|edit| edit.start);
    let mut end = 0;
    let mut next_len = previous_len;
    for edit in &delta.edits {
        let start = usize::try_from(edit.start).ok()?;
        let stop = start.checked_add(usize::try_from(edit.delete_count).ok()?)?;
        if start < end || stop > previous_len {
            return None;
        }
        next_len = next_len
            .checked_sub(stop - start)?
            .checked_add(edit.data.as_ref().map_or(0, Vec::len))?;
        end = stop;
    }
    if !next_len.is_multiple_of(5) {
        return None;
    }
    let mut next = Vec::with_capacity(next_len);
    let mut source = previous.iter().flat_map(values);
    end = 0;
    for edit in delta.edits {
        let start = edit.start as usize;
        next.extend(source.by_ref().take(start - end));
        next.extend(edit.data.unwrap_or_default());
        source
            .by_ref()
            .take(edit.delete_count as usize)
            .for_each(drop);
        end = start + edit.delete_count as usize;
    }
    next.extend(source);
    Some(lsp::SemanticTokens {
        result_id: delta.result_id,
        data: next
            .chunks_exact(5)
            .map(|values| lsp::SemanticToken {
                delta_line: values[0],
                delta_start: values[1],
                length: values[2],
                token_type: values[3],
                token_modifiers_bitset: values[4],
            })
            .collect(),
    })
}

fn values(token: &lsp::SemanticToken) -> [u32; 5] {
    [
        token.delta_line,
        token.delta_start,
        token.length,
        token.token_type,
        token.token_modifiers_bitset,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp::{SemanticTokensDelta as Delta, SemanticTokensEdit as Edit};

    fn token(values: [u32; 5]) -> lsp::SemanticToken {
        lsp::SemanticToken {
            delta_line: values[0],
            delta_start: values[1],
            length: values[2],
            token_type: values[3],
            token_modifiers_bitset: values[4],
        }
    }

    #[test]
    fn edits_use_integer_offsets_and_the_same_base_even_when_unsorted() {
        let previous = [token([0, 1, 3, 0, 0]), token([1, 2, 4, 0, 1])];
        let result = apply(
            &previous,
            Delta {
                result_id: Some("next".into()),
                edits: vec![
                    Edit {
                        start: 7,
                        delete_count: 1,
                        data: Some(vec![6]),
                    },
                    Edit {
                        start: 1,
                        delete_count: 1,
                        data: Some(vec![3]),
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(
            result.data,
            [token([0, 3, 3, 0, 0]), token([1, 2, 6, 0, 1])]
        );
        assert_eq!(result.result_id.as_deref(), Some("next"));
        assert_eq!(previous[0].delta_start, 1);
    }

    #[test]
    fn insertions_deletions_and_empty_deltas_preserve_the_full_array_shape() {
        let previous = [token([0, 1, 3, 0, 0]), token([1, 2, 4, 0, 1])];
        let result = apply(
            &previous,
            Delta {
                result_id: None,
                edits: vec![
                    Edit {
                        start: 5,
                        delete_count: 5,
                        data: None,
                    },
                    Edit {
                        start: 0,
                        delete_count: 0,
                        data: Some(vec![0, 0, 1, 0, 0]),
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(result.data, [token([0, 0, 1, 0, 0]), previous[0]]);
        assert_eq!(
            apply(
                &previous,
                Delta {
                    result_id: None,
                    edits: vec![]
                }
            )
            .unwrap()
            .data,
            previous
        );
        assert!(apply(
            &previous,
            Delta {
                result_id: None,
                edits: vec![Edit {
                    start: 0,
                    delete_count: 10,
                    data: None
                }]
            }
        )
        .unwrap()
        .data
        .is_empty());
    }

    #[test]
    fn invalid_bounds_overlaps_and_incomplete_token_arrays_are_rejected() {
        let previous = [token([0, 1, 3, 0, 0])];
        for edits in [
            vec![Edit {
                start: 6,
                delete_count: 0,
                data: None,
            }],
            vec![Edit {
                start: 4,
                delete_count: 2,
                data: None,
            }],
            vec![Edit {
                start: u32::MAX,
                delete_count: u32::MAX,
                data: None,
            }],
            vec![Edit {
                start: 0,
                delete_count: 1,
                data: None,
            }],
            vec![
                Edit {
                    start: 0,
                    delete_count: 3,
                    data: None,
                },
                Edit {
                    start: 2,
                    delete_count: 2,
                    data: None,
                },
            ],
        ] {
            assert!(apply(
                &previous,
                Delta {
                    result_id: None,
                    edits
                }
            )
            .is_none());
        }
    }
}
