//! Capability-filtered file creation, rename, and deletion messages.

use super::Client;
use crate::UriExt as _;
use crate::{
    lsp::{self, Url},
    Result,
};
use std::{future::Future, path::Path};

impl Client {
    fn file_operation_uri(path: &Path, is_dir: bool) -> Option<String> {
        let url = if is_dir {
            Url::from_directory_path(path)
        } else {
            Url::from_file_path(path)
        };
        Some(url.ok()?.to_string())
    }

    pub fn will_create(
        &self,
        path: &Path,
        is_dir: bool,
    ) -> Option<impl Future<Output = Result<Option<lsp::WorkspaceEdit>>> + use<>> {
        let capabilities = self.file_operations_interest();
        if !capabilities.will_create.has_interest(path, is_dir) {
            return None;
        }

        let files = vec![lsp::FileCreate {
            uri: Self::file_operation_uri(path, is_dir)?,
        }];
        Some(self.call_with_timeout::<lsp::request::WillCreateFiles>(
            &lsp::CreateFilesParams { files },
            5,
        ))
    }

    pub fn did_create(&self, path: &Path, is_dir: bool) -> Option<()> {
        let capabilities = self.file_operations_interest();
        if !capabilities.did_create.has_interest(path, is_dir) {
            return None;
        }

        let files = vec![lsp::FileCreate {
            uri: Self::file_operation_uri(path, is_dir)?,
        }];
        self.notify::<lsp::notification::DidCreateFiles>(lsp::CreateFilesParams { files });
        Some(())
    }

    pub fn will_rename(
        &self,
        old_path: &Path,
        new_path: &Path,
        is_dir: bool,
    ) -> Option<impl Future<Output = Result<Option<lsp::WorkspaceEdit>>> + use<>> {
        let capabilities = self.file_operations_interest();
        if !capabilities.will_rename.has_interest(old_path, is_dir) {
            return None;
        }
        let files = vec![lsp::FileRename {
            old_uri: Self::file_operation_uri(old_path, is_dir)?,
            new_uri: Self::file_operation_uri(new_path, is_dir)?,
        }];
        Some(self.call_with_timeout::<lsp::request::WillRenameFiles>(
            &lsp::RenameFilesParams { files },
            5,
        ))
    }

    pub fn did_rename(&self, old_path: &Path, new_path: &Path, is_dir: bool) -> Option<()> {
        let capabilities = self.file_operations_interest();
        if !capabilities.did_rename.has_interest(new_path, is_dir) {
            return None;
        }

        let files = vec![lsp::FileRename {
            old_uri: Self::file_operation_uri(old_path, is_dir)?,
            new_uri: Self::file_operation_uri(new_path, is_dir)?,
        }];
        self.notify::<lsp::notification::DidRenameFiles>(lsp::RenameFilesParams { files });
        Some(())
    }

    pub fn will_delete(
        &self,
        path: &Path,
        is_dir: bool,
    ) -> Option<impl Future<Output = Result<Option<lsp::WorkspaceEdit>>> + use<>> {
        let capabilities = self.file_operations_interest();
        if !capabilities.will_delete.has_interest(path, is_dir) {
            return None;
        }

        let files = vec![lsp::FileDelete {
            uri: Self::file_operation_uri(path, is_dir)?,
        }];
        Some(self.call_with_timeout::<lsp::request::WillDeleteFiles>(
            &lsp::DeleteFilesParams { files },
            5,
        ))
    }

    pub fn did_delete(&self, path: &Path, is_dir: bool) -> Option<()> {
        let capabilities = self.file_operations_interest();
        if !capabilities.did_delete.has_interest(path, is_dir) {
            return None;
        }

        let files = vec![lsp::FileDelete {
            uri: Self::file_operation_uri(path, is_dir)?,
        }];
        self.notify::<lsp::notification::DidDeleteFiles>(lsp::DeleteFilesParams { files });
        Some(())
    }

    // -------------------------------------------------------------------------------------------
    // Text document
    // -------------------------------------------------------------------------------------------
}
