//! Preserve each response family's protocol shape while appending batches.

use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;

use crate::{lsp, Error, Result};

pub(crate) trait PartialResponse: DeserializeOwned + Send + 'static {
    type Partial: DeserializeOwned + Send + 'static;
    type State: Default + Send;
    fn decode_partial(value: Value) -> Result<Self::Partial> {
        Ok(serde_json::from_value(value)?)
    }
    fn decode_response(value: Value) -> Result<Option<Self>> {
        Ok(serde_json::from_value(value)?)
    }
    fn append(state: &mut Self::State, batch: Self::Partial) -> Result<()>;
    fn finish(state: Self::State, response: Option<Self>) -> Result<Self>;
}

trait Accumulate: DeserializeOwned + Send + 'static {
    fn decode(value: Value) -> Result<Self> {
        Ok(serde_json::from_value(value)?)
    }
    fn append(&mut self, batch: Self) -> Result<()>;
}

impl<T: Accumulate> PartialResponse for Option<T> {
    type Partial = T;
    type State = Option<T>;
    fn decode_partial(value: Value) -> Result<T> {
        T::decode(value)
    }
    fn decode_response(value: Value) -> Result<Option<Self>> {
        if value.is_null() {
            Ok(None)
        } else {
            Ok(Some(Some(T::decode(value)?)))
        }
    }
    fn append(state: &mut Self::State, batch: T) -> Result<()> {
        if let Some(state) = state {
            state.append(batch)
        } else {
            *state = Some(batch);
            Ok(())
        }
    }
    fn finish(mut state: Self::State, response: Option<Self>) -> Result<Self> {
        if let Some(response) = response.flatten() {
            Self::append(&mut state, response)?;
        }
        Ok(state)
    }
}

impl<T: DeserializeOwned + Send + 'static> Accumulate for Vec<T> {
    fn append(&mut self, batch: Self) -> Result<()> {
        self.extend(batch);
        Ok(())
    }
}
impl<T: DeserializeOwned + Send + 'static> PartialResponse for Vec<T> {
    type Partial = Self;
    type State = Self;
    fn append(state: &mut Self, batch: Self) -> Result<()> {
        state.extend(batch);
        Ok(())
    }
    fn finish(mut state: Self, response: Option<Self>) -> Result<Self> {
        state.extend(response.into_iter().flatten());
        Ok(state)
    }
}

fn mixed_shapes() -> Error {
    anyhow::anyhow!("Language server mixed incompatible partial-result shapes").into()
}

impl Accumulate for lsp::CompletionResponse {
    fn append(&mut self, batch: Self) -> Result<()> {
        match (self, batch) {
            (Self::Array(items), Self::Array(batch)) => items.extend(batch),
            (Self::List(list), Self::Array(batch)) => list.items.extend(batch),
            // A list must be the first batch; its defaults apply to later arrays.
            (Self::List(_), Self::List(list)) if list.items.is_empty() => {}
            (Self::Array(_), Self::List(list)) if list.items.is_empty() => {}
            _ => return Err(mixed_shapes()),
        }
        Ok(())
    }
}

impl Accumulate for lsp::InlineCompletionResponse {
    fn append(&mut self, batch: Self) -> Result<()> {
        let batch = match batch {
            Self::Array(items) => items,
            Self::List(list) => list.items,
        };
        match self {
            Self::Array(items) => items.extend(batch),
            Self::List(list) => list.items.extend(batch),
        }
        Ok(())
    }
}

impl Accumulate for lsp::GotoDefinitionResponse {
    fn append(&mut self, batch: Self) -> Result<()> {
        if let Self::Scalar(location) = self {
            *self = Self::Array(vec![location.clone()]);
        }
        match (self, batch) {
            (Self::Array(items), Self::Scalar(item)) => items.push(item),
            (Self::Array(items), Self::Array(batch)) => items.extend(batch),
            (Self::Link(items), Self::Link(batch)) => items.extend(batch),
            (_, Self::Array(batch)) if batch.is_empty() => {}
            (_, Self::Link(batch)) if batch.is_empty() => {}
            (state @ Self::Array(_), batch @ Self::Link(_)) if matches!(&*state, Self::Array(items) if items.is_empty()) => {
                *state = batch
            }
            _ => return Err(mixed_shapes()),
        }
        Ok(())
    }
}

impl Accumulate for lsp::DocumentSymbolResponse {
    fn append(&mut self, batch: Self) -> Result<()> {
        match (self, batch) {
            (Self::Flat(items), Self::Flat(batch)) => items.extend(batch),
            (Self::Nested(items), Self::Nested(batch)) => items.extend(batch),
            (_, Self::Flat(batch)) if batch.is_empty() => {}
            (_, Self::Nested(batch)) if batch.is_empty() => {}
            (state @ Self::Flat(_), batch @ Self::Nested(_)) if matches!(&*state, Self::Flat(items) if items.is_empty()) => {
                *state = batch
            }
            _ => return Err(mixed_shapes()),
        }
        Ok(())
    }
}

impl Accumulate for lsp::WorkspaceSymbolResponse {
    fn decode(value: Value) -> Result<Self> {
        // WorkspaceSymbol also matches SymbolInformation when it has a range.
        // Decode the richer shape first so opaque resolve data is never erased.
        #[derive(Deserialize)]
        struct Symbol {
            #[serde(flatten)]
            symbol: lsp::WorkspaceSymbol,
            deprecated: Option<bool>,
        }
        let symbols: Vec<Symbol> = serde_json::from_value(value)?;
        Ok(Self::Nested(
            symbols
                .into_iter()
                .map(|mut item| {
                    if item.deprecated == Some(true) {
                        let tags = item.symbol.tags.get_or_insert_default();
                        if !tags.contains(&lsp::SymbolTag::DEPRECATED) {
                            tags.push(lsp::SymbolTag::DEPRECATED);
                        }
                    }
                    item.symbol
                })
                .collect(),
        ))
    }
    fn append(&mut self, batch: Self) -> Result<()> {
        match (self, batch) {
            (Self::Flat(items), Self::Flat(batch)) => items.extend(batch),
            (Self::Nested(items), Self::Nested(batch)) => items.extend(batch),
            _ => return Err(mixed_shapes()),
        }
        Ok(())
    }
}

/// Streaming fragments need not align with the five integers of one token.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawTokens {
    result_id: Option<String>,
    data: Vec<u32>,
}

impl RawTokens {
    fn from_tokens(tokens: lsp::SemanticTokens) -> Self {
        Self {
            result_id: tokens.result_id,
            data: tokens
                .data
                .into_iter()
                .flat_map(|token| {
                    [
                        token.delta_line,
                        token.delta_start,
                        token.length,
                        token.token_type,
                        token.token_modifiers_bitset,
                    ]
                })
                .collect(),
        }
    }

    fn append(&mut self, batch: Self) {
        self.data.extend(batch.data);
        if batch.result_id.is_some() {
            self.result_id = batch.result_id;
        }
    }

    fn into_tokens(self) -> Result<lsp::SemanticTokens> {
        let chunks = self.data.chunks_exact(5);
        if !chunks.remainder().is_empty() {
            return Err(
                anyhow::anyhow!("Semantic token result length is not divisible by five").into(),
            );
        }
        Ok(lsp::SemanticTokens {
            result_id: self.result_id,
            data: chunks
                .map(|chunk| lsp::SemanticToken {
                    delta_line: chunk[0],
                    delta_start: chunk[1],
                    length: chunk[2],
                    token_type: chunk[3],
                    token_modifiers_bitset: chunk[4],
                })
                .collect(),
        })
    }
}

fn append_tokens(state: &mut Option<RawTokens>, batch: RawTokens) {
    if let Some(state) = state {
        state.append(batch);
    } else {
        *state = Some(batch);
    }
}

macro_rules! semantic_tokens {
    ($ty:ident) => {
        impl PartialResponse for Option<lsp::$ty> {
            type Partial = RawTokens;
            type State = Option<RawTokens>;
            fn append(state: &mut Self::State, batch: RawTokens) -> Result<()> {
                append_tokens(state, batch);
                Ok(())
            }
            fn finish(mut state: Self::State, response: Option<Self>) -> Result<Self> {
                use lsp::$ty as Response;
                if let Some(response) = response.flatten() {
                    let tokens = match response {
                        Response::Tokens(tokens) => tokens,
                        Response::Partial(tokens) => lsp::SemanticTokens {
                            result_id: None,
                            data: tokens.data,
                        },
                    };
                    append_tokens(&mut state, RawTokens::from_tokens(tokens));
                }
                state
                    .map(|raw| raw.into_tokens().map(Response::Tokens))
                    .transpose()
            }
        }
    };
}
semantic_tokens!(SemanticTokensResult);
semantic_tokens!(SemanticTokensRangeResult);

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum SemanticDeltaBatch {
    Tokens(RawTokens),
    Edits(lsp::SemanticTokensDelta),
}

impl PartialResponse for Option<lsp::SemanticTokensFullDeltaResult> {
    type Partial = SemanticDeltaBatch;
    type State = Option<SemanticDeltaBatch>;
    fn append(state: &mut Self::State, batch: Self::Partial) -> Result<()> {
        match (state, batch) {
            (Some(SemanticDeltaBatch::Tokens(tokens)), SemanticDeltaBatch::Tokens(batch)) => {
                tokens.append(batch)
            }
            (Some(SemanticDeltaBatch::Edits(tokens)), SemanticDeltaBatch::Edits(batch)) => {
                tokens.edits.extend(batch.edits);
                if batch.result_id.is_some() {
                    tokens.result_id = batch.result_id;
                }
            }
            (state @ None, batch) => *state = Some(batch),
            _ => return Err(mixed_shapes()),
        }
        Ok(())
    }
    fn finish(mut state: Self::State, response: Option<Self>) -> Result<Self> {
        use lsp::SemanticTokensFullDeltaResult as Response;
        if let Some(response) = response.flatten() {
            let batch = match response {
                Response::Tokens(tokens) => {
                    SemanticDeltaBatch::Tokens(RawTokens::from_tokens(tokens))
                }
                Response::TokensDelta(delta) => SemanticDeltaBatch::Edits(delta),
                Response::PartialTokensDelta { edits } => {
                    SemanticDeltaBatch::Edits(lsp::SemanticTokensDelta {
                        result_id: None,
                        edits,
                    })
                }
            };
            Self::append(&mut state, batch)?;
        }
        state
            .map(|state| match state {
                SemanticDeltaBatch::Tokens(raw) => raw.into_tokens().map(Response::Tokens),
                SemanticDeltaBatch::Edits(delta) => Ok(Response::TokensDelta(delta)),
            })
            .transpose()
    }
}

#[derive(Default)]
pub(crate) struct DocumentDiagnostics {
    primary: Option<lsp::DocumentDiagnosticReport>,
    related: std::collections::HashMap<lsp::Uri, lsp::DocumentDiagnosticReportKind>,
}

impl PartialResponse for lsp::DocumentDiagnosticReport {
    type Partial = lsp::DocumentDiagnosticReportProgress;
    type State = DocumentDiagnostics;
    fn append(state: &mut Self::State, batch: Self::Partial) -> Result<()> {
        match batch {
            lsp::DocumentDiagnosticReportProgress::Report(mut report) => {
                if state.primary.is_some() {
                    return Err(mixed_shapes());
                }
                let related = match &mut report {
                    Self::Full(report) => &mut report.related_documents,
                    Self::Unchanged(report) => &mut report.related_documents,
                };
                if let Some(related) = related {
                    state.related.extend(std::mem::take(related));
                }
                state.primary = Some(report);
            }
            lsp::DocumentDiagnosticReportProgress::Partial(batch) => {
                state.related.extend(batch.related_documents);
            }
        }
        Ok(())
    }
    fn finish(mut state: Self::State, response: Option<Self>) -> Result<Self> {
        if let Some(response) = response {
            Self::append(
                &mut state,
                lsp::DocumentDiagnosticReportProgress::Report(response),
            )?;
        }
        let mut report = state.primary.ok_or_else(|| {
            Error::from(anyhow::anyhow!(
                "Document diagnostic request returned no primary report"
            ))
        })?;
        if !state.related.is_empty() {
            let related = match &mut report {
                Self::Full(report) => &mut report.related_documents,
                Self::Unchanged(report) => &mut report.related_documents,
            };
            related.get_or_insert_default().extend(state.related);
        }
        Ok(report)
    }
}

impl PartialResponse for lsp::WorkspaceDiagnosticReport {
    type Partial = Self;
    type State = Self;
    fn append(state: &mut Self, batch: Self) -> Result<()> {
        state.items.extend(batch.items);
        Ok(())
    }
    fn finish(mut state: Self, response: Option<Self>) -> Result<Self> {
        if let Some(response) = response {
            Self::append(&mut state, response)?;
        }
        Ok(state)
    }
}
