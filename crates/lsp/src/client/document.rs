//! Document synchronization and incremental change encoding.

use super::Client;
use crate::{lsp, OffsetEncoding};
use editor_core::{ChangeSet, Rope};
use stdx::rope::RopeExt as _;

impl Client {
    pub fn text_document_did_open(
        &self,
        uri: lsp::Url,
        version: i32,
        doc: &Rope,
        language_id: String,
    ) {
        self.notify::<lsp::notification::DidOpenTextDocument>(lsp::DidOpenTextDocumentParams {
            text_document: lsp::TextDocumentItem {
                uri,
                language_id,
                version,
                text: String::from(doc),
            },
        })
    }

    pub fn changeset_to_changes(
        old_text: &Rope,
        new_text: &Rope,
        changeset: &ChangeSet,
        offset_encoding: OffsetEncoding,
    ) -> Vec<lsp::TextDocumentContentChangeEvent> {
        let mut iter = changeset.changes().iter().peekable();
        let mut old_pos = 0;
        let mut new_pos = 0;

        let mut changes = Vec::new();

        use crate::util::pos_to_lsp_pos;
        use editor_core::Operation::*;

        // this is dumb. TextEdit describes changes to the initial doc (concurrent), but
        // TextDocumentContentChangeEvent describes a series of changes (sequential).
        // So S -> S1 -> S2, meaning positioning depends on the previous edits.
        //
        // Calculation is therefore a bunch trickier.

        use editor_core::RopeSlice;
        fn traverse(
            pos: lsp::Position,
            text: RopeSlice,
            offset_encoding: OffsetEncoding,
        ) -> lsp::Position {
            let lsp::Position {
                mut line,
                mut character,
            } = pos;

            let mut chars = text.chars().peekable();
            while let Some(ch) = chars.next() {
                // LSP only considers \n, \r or \r\n as line endings
                if ch == '\n' || ch == '\r' {
                    // consume a \r\n
                    if ch == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    line += 1;
                    character = 0;
                } else {
                    character += match offset_encoding {
                        OffsetEncoding::Utf8 => ch.len_utf8() as u32,
                        OffsetEncoding::Utf16 => ch.len_utf16() as u32,
                        OffsetEncoding::Utf32 => 1,
                    };
                }
            }
            lsp::Position { line, character }
        }

        let old_text = old_text.slice(..);

        while let Some(change) = iter.next() {
            let len = match change {
                Delete(i) | Retain(i) => *i,
                Insert(_) => 0,
            };
            let mut old_end = old_pos + len;

            match change {
                Retain(i) => {
                    new_pos += i;
                }
                Delete(_) => {
                    let start = pos_to_lsp_pos(new_text, new_pos, offset_encoding);
                    let end = traverse(
                        start,
                        old_text.slice_chars(old_pos..old_end),
                        offset_encoding,
                    );

                    // deletion
                    changes.push(lsp::TextDocumentContentChangeEvent {
                        range: Some(lsp::Range::new(start, end)),
                        text: "".to_string(),
                        range_length: None,
                    });
                }
                Insert(s) => {
                    let start = pos_to_lsp_pos(new_text, new_pos, offset_encoding);

                    new_pos += s.chars().count();

                    // a subsequent delete means a replace, consume it
                    let end = if let Some(Delete(len)) = iter.peek() {
                        old_end = old_pos + len;
                        let end = traverse(
                            start,
                            old_text.slice_chars(old_pos..old_end),
                            offset_encoding,
                        );

                        iter.next();

                        // replacement
                        end
                    } else {
                        // insert
                        start
                    };

                    changes.push(lsp::TextDocumentContentChangeEvent {
                        range: Some(lsp::Range::new(start, end)),
                        text: s.to_string(),
                        range_length: None,
                    });
                }
            }
            old_pos = old_end;
        }

        changes
    }

    pub fn text_document_did_change(
        &self,
        text_document: lsp::VersionedTextDocumentIdentifier,
        old_text: &Rope,
        new_text: &Rope,
        changes: &ChangeSet,
    ) -> Option<()> {
        let capabilities = self.capabilities.get().unwrap();

        // Return early if the server does not support document sync.
        let sync_capabilities = match capabilities.text_document_sync {
            Some(
                lsp::TextDocumentSyncCapability::Kind(kind)
                | lsp::TextDocumentSyncCapability::Options(lsp::TextDocumentSyncOptions {
                    change: Some(kind),
                    ..
                }),
            ) => kind,
            // None | SyncOptions { changes: None }
            _ => return None,
        };

        let changes = match sync_capabilities {
            lsp::TextDocumentSyncKind::FULL => {
                vec![lsp::TextDocumentContentChangeEvent {
                    // range = None -> whole document
                    range: None,        //Some(Range)
                    range_length: None, // u64 apparently deprecated
                    text: new_text.to_string(),
                }]
            }
            lsp::TextDocumentSyncKind::INCREMENTAL => {
                Self::changeset_to_changes(old_text, new_text, changes, self.offset_encoding())
            }
            lsp::TextDocumentSyncKind::NONE => return None,
            kind => unimplemented!("{:?}", kind),
        };

        self.notify::<lsp::notification::DidChangeTextDocument>(lsp::DidChangeTextDocumentParams {
            text_document,
            content_changes: changes,
        });
        Some(())
    }

    pub fn text_document_did_close(&self, text_document: lsp::TextDocumentIdentifier) {
        self.notify::<lsp::notification::DidCloseTextDocument>(lsp::DidCloseTextDocumentParams {
            text_document,
        })
    }

    // will_save / will_save_wait_until

    pub fn text_document_did_save(
        &self,
        text_document: lsp::TextDocumentIdentifier,
        text: &Rope,
    ) -> Option<()> {
        let capabilities = self.capabilities.get().unwrap();

        let include_text = match &capabilities.text_document_sync.as_ref()? {
            lsp::TextDocumentSyncCapability::Options(lsp::TextDocumentSyncOptions {
                save: options,
                ..
            }) => match options.as_ref()? {
                lsp::TextDocumentSyncSaveOptions::Supported(true) => false,
                lsp::TextDocumentSyncSaveOptions::SaveOptions(lsp::SaveOptions {
                    include_text,
                }) => include_text.unwrap_or(false),
                lsp::TextDocumentSyncSaveOptions::Supported(false) => return None,
            },
            // see: https://github.com/microsoft/language-server-protocol/issues/288
            lsp::TextDocumentSyncCapability::Kind(..) => false,
        };

        self.notify::<lsp::notification::DidSaveTextDocument>(lsp::DidSaveTextDocumentParams {
            text_document,
            text: include_text.then_some(text.into()),
        });
        Some(())
    }
}
