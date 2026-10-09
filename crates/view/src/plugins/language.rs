//! Owned syntax/attached-language replies; all expensive work stays off editor callbacks.
use super::*;
use editor_core::{
    syntax::config::LanguageServerFeature,
    tree_sitter::{query::InvalidPredicateError, InactiveQueryCursor, Query, RopeInput, Tree},
};
use futures_util::future::BoxFuture;
use lsp_client::{
    lsp,
    util::{lsp_pos_to_pos, pos_to_lsp_pos},
    LanguageServerId, OffsetEncoding,
};
use plugin_api::{
    editor::{
        DocumentTarget, EditorReply, EditorRequest, LanguageSymbol, SyntaxCapture, TextRange,
        LANGUAGE_DEADLINE_MILLIS, MAX_EDITOR_REPLY_BYTES,
    },
    HostFuture,
};
use std::{
    io::Write,
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, Semaphore};

const MAX_DOCUMENT_BYTES: usize = 128 * 1024 * 1024;
const MAX_SYNTAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_RANGE_BYTES: usize = 1024 * 1024;
const MAX_PATTERNS: usize = plugin_api::query::STRUCTURAL_LIMITS.patterns;
const MAX_MATCHES: usize = 4096;
const MAX_HOVER_BYTES: usize = 64 * 1024;

fn failure(code: ErrorCode, message: impl Into<String>) -> ServiceError {
    ServiceError::new(code, message)
}
fn unsupported(message: &str) -> ServiceError {
    failure(ErrorCode::UnsupportedInterface, message)
}
fn exhausted(message: &str) -> ServiceError {
    failure(ErrorCode::ResourceExhausted, message)
}
fn stale(message: &str) -> ServiceError {
    failure(ErrorCode::StaleState, message)
}
fn cancelled() -> ServiceError {
    failure(ErrorCode::Cancelled, "plugin language request revoked")
}
fn language_failure(error: lsp_client::Error) -> ServiceError {
    let code = match error {
        lsp_client::Error::RequestLimit => ErrorCode::ResourceExhausted,
        lsp_client::Error::Timeout(_) => ErrorCode::DeadlineExceeded,
        _ => ErrorCode::HostFailure,
    };
    failure(code, error.to_string())
}

pub(super) fn handles(request: &EditorRequest) -> bool {
    matches!(
        request,
        EditorRequest::SyntaxQuery { .. }
            | EditorRequest::LanguageHover { .. }
            | EditorRequest::LanguageSymbols { .. }
    )
}

pub(super) fn request(
    request: EditorRequest,
    owner: Weak<Shared>,
    callbacks: EditorCallbackSender,
    policy: Arc<::plugins::policy::AccessPolicy>,
    work: Arc<Semaphore>,
) -> HostFuture<EditorReply> {
    Box::pin(async move {
        request.validate()?;
        if let EditorRequest::SyntaxQuery { query, .. } = &request {
            validate_query(query)?;
        }
        let target = match &request {
            EditorRequest::SyntaxQuery { target, .. }
            | EditorRequest::LanguageHover { target, .. }
            | EditorRequest::LanguageSymbols { target, .. } => *target,
            _ => return Err(unsupported("not a syntax or language request")),
        };
        let shared = owner.upgrade().ok_or_else(cancelled)?;
        policy.require(Capability::EditorRead)?;
        if !shared.accepting.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        let deadline = Instant::now() + Duration::from_millis(LANGUAGE_DEADLINE_MILLIS);
        tokio::time::timeout_at(deadline.into(), async {
            // Moving this permit into native syntax work keeps admission bounded
            // even after a timed-out caller drops its blocking-task handle.
            let permit = work.acquire_owned().await.map_err(|_| cancelled())?;
            let (send, receive) = oneshot::channel();
            let weak = owner.clone();
            let granted = policy.clone();
            callbacks
                .send(move |editor| {
                    if send.is_closed() {
                        return;
                    }
                    let result = (|| {
                        live(&weak, editor, &granted)?;
                        capture(editor, target, request)
                    })();
                    let _ = send.send(result);
                })
                .await;
            let captured = receive.await.map_err(|_| cancelled())??;
            let check = captured.check();
            let reply = match captured {
                Captured::Syntax {
                    text,
                    tree,
                    range,
                    query,
                    limit,
                    ..
                } => tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    query_captures(target, text, tree, range, &query, limit, deadline)
                })
                .await
                .map_err(|cause| failure(ErrorCode::HostFailure, cause.to_string()))??,
                Captured::Hover {
                    text,
                    encoding,
                    offset,
                    future,
                    ..
                } => {
                    let _permit = permit;
                    let hover = future.await.map_err(language_failure)?;
                    hover_reply(target, offset, &text, encoding, hover)?
                }
                Captured::Symbols {
                    text,
                    encoding,
                    uri,
                    future,
                    limit,
                    ..
                } => {
                    let _permit = permit;
                    let symbols = future.await.map_err(language_failure)?;
                    symbol_reply(target, &text, encoding, &uri, symbols, limit)?
                }
            };
            validate_reply_bytes(&reply)?;
            let (send, receive) = oneshot::channel();
            let weak = owner.clone();
            let granted = policy.clone();
            callbacks
                .send(move |editor| {
                    let _ = send.send((|| {
                        live(&weak, editor, &granted)?;
                        check_current(editor, target, check)
                    })());
                })
                .await;
            receive.await.map_err(|_| cancelled())??;
            Ok(reply)
        })
        .await
        .map_err(|_| {
            failure(
                ErrorCode::DeadlineExceeded,
                "plugin language service exceeded two seconds",
            )
        })?
    })
}
fn live(
    owner: &Weak<Shared>,
    editor: &Editor,
    policy: &::plugins::policy::AccessPolicy,
) -> Result<(), ServiceError> {
    let shared = owner.upgrade().ok_or_else(cancelled)?;
    if !Arc::ptr_eq(&shared, &editor.plugins.shared)
        || !shared.accepting.load(Ordering::Acquire)
        || editor.plugins.stopped
    {
        return Err(cancelled());
    }
    policy.require(Capability::EditorRead)
}

enum Captured {
    Syntax {
        text: Rope,
        tree: Tree,
        range: std::ops::Range<u32>,
        query: String,
        limit: usize,
    },
    Hover {
        text: Rope,
        encoding: OffsetEncoding,
        offset: u64,
        server: LanguageServerId,
        future: BoxFuture<'static, lsp_client::Result<Option<lsp::Hover>>>,
    },
    Symbols {
        text: Rope,
        encoding: OffsetEncoding,
        uri: lsp::Url,
        server: LanguageServerId,
        future: BoxFuture<'static, lsp_client::Result<Option<lsp::DocumentSymbolResponse>>>,
        limit: usize,
    },
}
#[derive(Clone, Copy)]
enum CompletionCheck {
    Syntax(editor_core::tree_sitter::Grammar),
    Server(LanguageServerId, LanguageServerFeature),
}
impl Captured {
    fn check(&self) -> CompletionCheck {
        match self {
            Self::Syntax { tree, .. } => CompletionCheck::Syntax(tree.root_node().grammar()),
            Self::Hover { server, .. } => {
                CompletionCheck::Server(*server, LanguageServerFeature::Hover)
            }
            Self::Symbols { server, .. } => {
                CompletionCheck::Server(*server, LanguageServerFeature::DocumentSymbols)
            }
        }
    }
}
fn document(editor: &Editor, target: DocumentTarget) -> Result<&Document, ServiceError> {
    let doc = editor
        .documents
        .values()
        .find(|doc| doc.id().as_u64() == target.document)
        .ok_or_else(|| stale("language target is closed"))?;
    if doc.version() != target.version {
        return Err(stale("language target version changed"));
    }
    if doc.text().len_bytes() > MAX_DOCUMENT_BYTES {
        return Err(exhausted("language source snapshot exceeds 128 MiB"));
    }
    Ok(doc)
}
fn capture(
    editor: &Editor,
    target: DocumentTarget,
    request: EditorRequest,
) -> Result<Captured, ServiceError> {
    let doc = document(editor, target)?;
    match request {
        EditorRequest::SyntaxQuery {
            range,
            query,
            max_captures,
            ..
        } => {
            if doc.text().len_bytes() > MAX_SYNTAX_DOCUMENT_BYTES {
                return Err(exhausted("syntax query documents are limited to 16 MiB"));
            }
            let start =
                usize::try_from(range.start).map_err(|_| stale("query offset exceeds document"))?;
            let end =
                usize::try_from(range.end).map_err(|_| stale("query offset exceeds document"))?;
            if start > end || end > doc.text().len_chars() {
                return Err(stale("query range exceeds document"));
            }
            let start = doc.text().char_to_byte(start);
            let end = doc.text().char_to_byte(end);
            if end - start > MAX_RANGE_BYTES {
                return Err(exhausted("syntax query range exceeds 1 MiB"));
            }
            let syntax = doc
                .syntax()
                .ok_or_else(|| unsupported("document has no existing syntax tree"))?;
            Ok(Captured::Syntax {
                text: doc.text().clone(),
                tree: syntax.tree().clone(),
                range: start as u32..end as u32,
                query,
                limit: if max_captures == 0 {
                    256
                } else {
                    max_captures as usize
                },
            })
        }
        EditorRequest::LanguageHover { offset, server, .. } => {
            check_server_name(server.as_deref())?;
            let offset_native =
                usize::try_from(offset).map_err(|_| stale("hover offset exceeds document"))?;
            if offset_native > doc.text().len_chars() {
                return Err(stale("hover offset exceeds document"));
            }
            let client = doc
                .language_servers_with_feature(LanguageServerFeature::Hover)
                .find(|client| server.as_ref().is_none_or(|name| client.name() == name))
                .ok_or_else(|| unsupported("no attached initialized hover server matches"))?;
            let uri = doc
                .url()
                .ok_or_else(|| unsupported("language target has no document URI"))?;
            let encoding = client.offset_encoding();
            let position = pos_to_lsp_pos(doc.text(), offset_native, encoding);
            let future = client
                .text_document_hover_cancellable(lsp::TextDocumentIdentifier::new(uri), position)
                .map_err(language_failure)?
                .ok_or_else(|| unsupported("attached server does not support hover"))?;
            Ok(Captured::Hover {
                text: doc.text().clone(),
                encoding,
                offset,
                server: client.id(),
                future: Box::pin(future),
            })
        }
        EditorRequest::LanguageSymbols {
            server,
            max_symbols,
            ..
        } => {
            check_server_name(server.as_deref())?;
            let client = doc
                .language_servers_with_feature(LanguageServerFeature::DocumentSymbols)
                .find(|client| server.as_ref().is_none_or(|name| client.name() == name))
                .ok_or_else(|| unsupported("no attached initialized symbol server matches"))?;
            let uri = doc
                .url()
                .ok_or_else(|| unsupported("language target has no document URI"))?;
            let future = client
                .document_symbols_cancellable(lsp::TextDocumentIdentifier::new(uri.clone()))
                .map_err(language_failure)?
                .ok_or_else(|| unsupported("attached server does not support symbols"))?;
            Ok(Captured::Symbols {
                text: doc.text().clone(),
                encoding: client.offset_encoding(),
                uri,
                server: client.id(),
                future: Box::pin(future),
                limit: if max_symbols == 0 {
                    256
                } else {
                    max_symbols as usize
                },
            })
        }
        _ => Err(unsupported("not a syntax or language request")),
    }
}
fn check_server_name(server: Option<&str>) -> Result<(), ServiceError> {
    if server.is_some_and(|name| {
        name.is_empty() || name.len() > 128 || name.chars().any(char::is_control)
    }) {
        return Err(failure(
            ErrorCode::InvalidRequest,
            "server name must be bounded text",
        ));
    }
    Ok(())
}
fn check_current(
    editor: &Editor,
    target: DocumentTarget,
    check: CompletionCheck,
) -> Result<(), ServiceError> {
    let doc = document(editor, target)?;
    match check {
        CompletionCheck::Syntax(grammar)
            if doc
                .syntax()
                .is_none_or(|syntax| syntax.tree().root_node().grammar() != grammar) =>
        {
            Err(stale("syntax grammar changed"))
        }
        CompletionCheck::Server(id, feature)
            if !doc
                .language_servers_with_feature(feature)
                .any(|client| client.id() == id) =>
        {
            Err(stale("language server is no longer attached"))
        }
        _ => Ok(()),
    }
}

/// Structural queries avoid unbounded text predicates. The native cursor has no
/// progress/timeout hook; bounds and admission remain held beyond caller timeout.
fn validate_query(query: &str) -> Result<(), ServiceError> {
    if query.is_empty() {
        return Err(failure(
            ErrorCode::InvalidRequest,
            "syntax query source is empty",
        ));
    }
    plugin_api::query::validate(
        query,
        plugin_api::query::STRUCTURAL_LIMITS,
        plugin_api::query::Predicates::Structural,
    )
}

fn query_captures(
    target: DocumentTarget,
    text: Rope,
    tree: Tree,
    range: std::ops::Range<u32>,
    source: &str,
    limit: usize,
    deadline: Instant,
) -> Result<EditorReply, ServiceError> {
    validate_query(source)?;
    let query = Query::new(tree.root_node().grammar(), source, |_, predicate| {
        Err(InvalidPredicateError::unknown(predicate))
    })
    .map_err(|cause| failure(ErrorCode::InvalidRequest, cause.to_string()))?;
    if query.patterns().len() > MAX_PATTERNS
        || query.captures().len() > 256
        || query.captures().any(|(_, name)| name.len() > 128)
    {
        return Err(exhausted("syntax query pattern/capture limits exceeded"));
    }
    let root = tree.root_node();
    let mut cursor = InactiveQueryCursor::new(range.clone(), 256).execute_query(
        &query,
        &root,
        RopeInput::new(text.slice(..)),
    );
    let mut captures = Vec::new();
    let mut count = 0;
    let mut truncated = false;
    while let Some((matched, index)) = cursor.next_matched_node() {
        if Instant::now() >= deadline {
            return Err(failure(
                ErrorCode::DeadlineExceeded,
                "native syntax query exceeded its deadline",
            ));
        }
        count += 1;
        if count > MAX_MATCHES {
            truncated = true;
            break;
        }
        let captured = matched.matched_node(index);
        let bytes = captured.node.byte_range();
        if bytes.start < range.start || bytes.end > range.end {
            continue;
        }
        if captures.len() == limit {
            truncated = true;
            break;
        }
        captures.push(SyntaxCapture {
            name: query.capture_name(captured.capture).into(),
            range: TextRange {
                start: text.byte_to_char(bytes.start as usize) as u64,
                end: text.byte_to_char(bytes.end as usize) as u64,
            },
        });
    }
    truncated |= cursor.reuse().did_exceed_match_limit();
    Ok(EditorReply::Syntax {
        target,
        captures,
        truncated,
    })
}
fn strict_range(
    text: &Rope,
    range: lsp::Range,
    encoding: OffsetEncoding,
) -> Result<TextRange, ServiceError> {
    if range.start > range.end || range.end.line as usize >= text.len_lines() {
        return Err(failure(
            ErrorCode::HostFailure,
            "language server returned an invalid range",
        ));
    }
    let start = lsp_pos_to_pos(text, range.start, encoding)
        .ok_or_else(|| failure(ErrorCode::HostFailure, "invalid language start position"))?;
    let end = lsp_pos_to_pos(text, range.end, encoding)
        .ok_or_else(|| failure(ErrorCode::HostFailure, "invalid language end position"))?;
    // Existing conversion deliberately clamps LSP positions; require an exact
    // round trip for this explicit owned service instead of accepting truncation.
    if pos_to_lsp_pos(text, start, encoding) != range.start
        || pos_to_lsp_pos(text, end, encoding) != range.end
    {
        return Err(failure(
            ErrorCode::HostFailure,
            "language positions are out of bounds",
        ));
    }
    Ok(TextRange {
        start: start as u64,
        end: end as u64,
    })
}
fn marked(contents: lsp::MarkedString) -> Result<String, ServiceError> {
    match contents {
        lsp::MarkedString::String(value) => {
            if value.len() > MAX_HOVER_BYTES {
                return Err(exhausted("hover content exceeds 64 KiB"));
            }
            Ok(value)
        }
        lsp::MarkedString::LanguageString(value) => {
            if value.value.len() + value.language.len() + 10 > MAX_HOVER_BYTES {
                return Err(exhausted("hover content exceeds 64 KiB"));
            }
            Ok(if value.language == "markdown" {
                value.value
            } else {
                format!("```{}\n{}\n```", value.language, value.value)
            })
        }
    }
}
fn hover_reply(
    target: DocumentTarget,
    offset: u64,
    text: &Rope,
    encoding: OffsetEncoding,
    hover: Option<lsp::Hover>,
) -> Result<EditorReply, ServiceError> {
    let (markdown, range) = if let Some(hover) = hover {
        let markdown = match hover.contents {
            lsp::HoverContents::Scalar(value) => marked(value)?,
            lsp::HoverContents::Markup(value) => {
                if value.value.len() > MAX_HOVER_BYTES {
                    return Err(exhausted("hover content exceeds 64 KiB"));
                }
                value.value
            }
            lsp::HoverContents::Array(values) => {
                let mut text = String::new();
                for value in values {
                    let value = marked(value)?;
                    if text.len().saturating_add(value.len() + 2) > MAX_HOVER_BYTES {
                        return Err(exhausted("hover content exceeds 64 KiB"));
                    }
                    if !text.is_empty() {
                        text.push_str("\n\n");
                    }
                    text.push_str(&value);
                }
                text
            }
        };
        (
            markdown,
            hover
                .range
                .map(|range| strict_range(text, range, encoding))
                .transpose()?,
        )
    } else {
        (String::new(), None)
    };
    Ok(EditorReply::Hover {
        target,
        offset,
        markdown,
        range,
    })
}
fn symbol_reply(
    target: DocumentTarget,
    text: &Rope,
    encoding: OffsetEncoding,
    uri: &lsp::Url,
    response: Option<lsp::DocumentSymbolResponse>,
    limit: usize,
) -> Result<EditorReply, ServiceError> {
    let mut symbols = Vec::new();
    let mut truncated = false;
    match response {
        Some(lsp::DocumentSymbolResponse::Flat(values)) => {
            for value in values {
                if symbols.len() == limit {
                    truncated = true;
                    break;
                }
                if &value.location.uri != uri {
                    continue;
                }
                check_symbol_text(&value.name, None)?;
                let range = strict_range(text, value.location.range, encoding)?;
                symbols.push(LanguageSymbol {
                    name: value.name,
                    detail: None,
                    kind: format!("{:?}", value.kind),
                    range,
                    selection: range,
                    parent: None,
                });
            }
        }
        Some(lsp::DocumentSymbolResponse::Nested(values)) => {
            let mut stack = vec![(values.into_iter(), None)];
            while let Some((values, parent)) = stack.last_mut() {
                let Some(value) = values.next() else {
                    stack.pop();
                    continue;
                };
                if symbols.len() == limit {
                    truncated = true;
                    break;
                }
                check_symbol_text(&value.name, value.detail.as_deref())?;
                let index = symbols.len() as u32;
                let range = strict_range(text, value.range, encoding)?;
                let selection = strict_range(text, value.selection_range, encoding)?;
                if selection.start < range.start || selection.end > range.end {
                    return Err(failure(
                        ErrorCode::HostFailure,
                        "symbol selection exceeds symbol range",
                    ));
                }
                symbols.push(LanguageSymbol {
                    name: value.name,
                    detail: value.detail,
                    kind: format!("{:?}", value.kind),
                    range,
                    selection,
                    parent: *parent,
                });
                if let Some(children) = value.children {
                    if stack.len() >= 64 {
                        return Err(exhausted("symbol nesting exceeds 64"));
                    }
                    stack.push((children.into_iter(), Some(index)));
                }
            }
        }
        None => (),
    }
    Ok(EditorReply::Symbols {
        target,
        symbols,
        truncated,
    })
}
fn check_symbol_text(name: &str, detail: Option<&str>) -> Result<(), ServiceError> {
    if name.len() > 4096 || detail.is_some_and(|value| value.len() > 4096) {
        return Err(exhausted("symbol text exceeds 4 KiB"));
    }
    Ok(())
}
struct ReplyBytes(usize);
impl Write for ReplyBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        if self.0 > MAX_EDITOR_REPLY_BYTES {
            return Err(std::io::Error::other("editor reply limit exceeded"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn validate_reply_bytes(reply: &EditorReply) -> Result<(), ServiceError> {
    serde_json::to_writer(&mut ReplyBytes(0), reply)
        .map_err(|_| exhausted("language reply exceeds 1 MiB"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> DocumentTarget {
        DocumentTarget {
            document: 1,
            version: 7,
        }
    }

    #[test]
    fn structural_queries_return_unicode_offsets_and_truncate() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
        let grammar = loader::syntax::Resources::new(vec![root])
            .grammar("rust")
            .unwrap()
            .expect("Rust grammar fixture must be built");
        let text = Rope::from_str("fn main() { let café = 1; }\n");
        let mut parser = editor_core::tree_sitter::Parser::new();
        parser.set_grammar(grammar).unwrap();
        let tree = parser
            .parse_with_timeout(RopeInput::new(text.slice(..)), None, Duration::from_secs(1))
            .unwrap();
        let run = |source: &str, limit| {
            query_captures(
                target(),
                text.clone(),
                tree.clone(),
                0..text.len_bytes() as u32,
                source,
                limit,
                Instant::now() + Duration::from_secs(2),
            )
        };
        let EditorReply::Syntax {
            captures,
            truncated,
            ..
        } = run("(identifier) @name", 256).unwrap()
        else {
            panic!("not syntax")
        };
        assert!(!truncated);
        assert_eq!(
            captures
                .iter()
                .map(|capture| text
                    .slice(capture.range.start as usize..capture.range.end as usize)
                    .to_string())
                .collect::<Vec<_>>(),
            ["main", "café"]
        );
        let EditorReply::Syntax {
            captures,
            truncated,
            ..
        } = run("(identifier) @name", 1).unwrap()
        else {
            panic!("not syntax")
        };
        assert_eq!(captures.len(), 1);
        assert!(truncated);
        assert_eq!(
            run(&"(identifier) @name\n".repeat(65), 256)
                .unwrap_err()
                .code,
            ErrorCode::ResourceExhausted
        );
    }

    #[test]
    fn unsupported_predicates_and_excessive_query_complexity_fail_before_compile() {
        assert_eq!(
            validate_query("((identifier) @x (#eq? @x \"value\"))")
                .unwrap_err()
                .code,
            ErrorCode::UnsupportedInterface
        );
        assert!(validate_query("; #eq? inside comment\n(\"#\") @x").is_ok());
        assert_eq!(
            validate_query(&"(".repeat(65)).unwrap_err().code,
            ErrorCode::ResourceExhausted
        );
        assert_eq!(
            validate_query(&"x".repeat(4097)).unwrap_err().code,
            ErrorCode::ResourceExhausted
        );
    }

    #[test]
    fn hover_offsets_reject_malformed_utf16_and_bound_owned_markdown() {
        let text = Rope::from_str("😀abc\n");
        let range =
            |column| lsp::Range::new(lsp::Position::new(0, column), lsp::Position::new(0, column));
        assert_eq!(
            strict_range(&text, range(2), OffsetEncoding::Utf16).unwrap(),
            TextRange { start: 1, end: 1 }
        );
        assert!(strict_range(&text, range(1), OffsetEncoding::Utf16).is_err());
        assert!(strict_range(&text, range(99), OffsetEncoding::Utf16).is_err());
        let hover = lsp::Hover {
            contents: lsp::HoverContents::Markup(lsp::MarkupContent {
                kind: lsp::MarkupKind::Markdown,
                value: "x".repeat(MAX_HOVER_BYTES + 1),
            }),
            range: None,
        };
        assert_eq!(
            hover_reply(target(), 1, &text, OffsetEncoding::Utf16, Some(hover))
                .unwrap_err()
                .code,
            ErrorCode::ResourceExhausted
        );
    }

    #[test]
    fn nested_symbols_keep_owned_parent_indices_and_validate_ranges() {
        let text = Rope::from_str("abcd\n");
        let uri = lsp::Url::parse("file:///test.rs").unwrap();
        let symbol = |name: &str, start, end, children| {
            serde_json::from_value::<lsp::DocumentSymbol>(serde_json::json!({
            "name": name, "kind": 12, "range": {"start":{"line":0,"character":start},"end":{"line":0,"character":end}},
            "selectionRange":{"start":{"line":0,"character":start},"end":{"line":0,"character":end}}, "children":children
        })).unwrap()
        };
        let child = symbol("child", 1, 2, serde_json::Value::Null);
        let parent = symbol("parent", 0, 4, serde_json::to_value(&[child]).unwrap());
        let run = |limit| {
            symbol_reply(
                target(),
                &text,
                OffsetEncoding::Utf8,
                &uri,
                Some(lsp::DocumentSymbolResponse::Nested(vec![parent.clone()])),
                limit,
            )
        };
        let EditorReply::Symbols {
            symbols, truncated, ..
        } = run(256).unwrap()
        else {
            panic!("not symbols")
        };
        assert!(!truncated);
        assert_eq!(
            symbols
                .iter()
                .map(|symbol| symbol.parent)
                .collect::<Vec<_>>(),
            [None, Some(0)]
        );
        assert!(matches!(
            run(1).unwrap(),
            EditorReply::Symbols {
                truncated: true,
                ..
            }
        ));
        let malformed = symbol("bad", 0, 99, serde_json::Value::Null);
        assert!(symbol_reply(
            target(),
            &text,
            OffsetEncoding::Utf8,
            &uri,
            Some(lsp::DocumentSymbolResponse::Nested(vec![malformed])),
            256
        )
        .is_err());
    }
}
