//! Stream UI/composition models into the same atomic, bounded effect batch.
use super::*;
use plugin_api::ui::*;

fn origin(value: types::UiOrigin) -> UiOrigin {
    UiOrigin {
        view: value.view,
        document: value.document,
        binding_revision: value.binding_revision,
        version: value.version,
        selection_revision: value.selection_revision,
    }
}
fn builtin(value: types::BuiltinCommand) -> BuiltinCommand {
    match value {
        types::BuiltinCommand::MoveCharLeft => BuiltinCommand::MoveCharLeft,
        types::BuiltinCommand::MoveCharRight => BuiltinCommand::MoveCharRight,
        types::BuiltinCommand::MoveLineUp => BuiltinCommand::MoveLineUp,
        types::BuiltinCommand::MoveLineDown => BuiltinCommand::MoveLineDown,
        types::BuiltinCommand::SelectAll => BuiltinCommand::SelectAll,
        types::BuiltinCommand::CollapseSelection => BuiltinCommand::CollapseSelection,
        types::BuiltinCommand::KeepPrimarySelection => BuiltinCommand::KeepPrimarySelection,
        types::BuiltinCommand::DeleteSelectionNoYank => BuiltinCommand::DeleteSelectionNoYank,
        types::BuiltinCommand::ChangeSelectionNoYank => BuiltinCommand::ChangeSelectionNoYank,
        types::BuiltinCommand::Undo => BuiltinCommand::Undo,
        types::BuiltinCommand::Redo => BuiltinCommand::Redo,
        types::BuiltinCommand::InsertMode => BuiltinCommand::InsertMode,
        types::BuiltinCommand::NormalMode => BuiltinCommand::NormalMode,
    }
}
impl HostState {
    pub(super) fn ui_prompt(
        &mut self,
        batch: Resource<EffectsHandle>,
        request: u64,
        target: Option<types::UiOrigin>,
        title: String,
    ) -> Result<Resource<PromptHandle>, ServiceError> {
        self.builder(self.table.get(&batch).map_err(|_| stale())?.invocation)?;
        self.require(Capability::Ui)?;
        if title.len() > MAX_UI_TITLE_BYTES {
            return Err(exhausted("plugin prompt title exceeds its size limit"));
        }
        let bytes = title.len();
        let handle = self
            .table
            .push(PromptHandle {
                invocation: self.invocation,
                action: self.response.actions.len(),
                initial_set: false,
                finished: false,
            })
            .map_err(|_| exhausted("plugin resource handle count exceeded"))?;
        if let Err(error) = self.action(
            Action::ShowUi {
                request,
                origin: target.map(origin),
                kind: UiKind::Prompt {
                    title,
                    initial: String::new(),
                },
            },
            bytes,
        ) {
            let _ = self.table.delete(handle);
            return Err(error);
        }
        self.open_groups += 1;
        Ok(handle)
    }
    pub(super) fn ui_next_key(
        &mut self,
        batch: Resource<EffectsHandle>,
        request: u64,
        target: Option<types::UiOrigin>,
        title: String,
        timeout_ms: u32,
    ) -> Result<(), ServiceError> {
        self.builder(self.table.get(&batch).map_err(|_| stale())?.invocation)?;
        self.require(Capability::Ui)?;
        if title.len() > MAX_UI_TITLE_BYTES || !(1..=60_000).contains(&timeout_ms) {
            return Err(exhausted("plugin key request exceeds its bounds"));
        }
        let bytes = title.len();
        self.action(
            Action::ShowUi {
                request,
                origin: target.map(origin),
                kind: UiKind::NextKey { title, timeout_ms },
            },
            bytes,
        )?;
        Ok(())
    }
    pub(super) fn ui_picker(
        &mut self,
        batch: Resource<EffectsHandle>,
        request: u64,
        target: Option<types::UiOrigin>,
        title: String,
    ) -> Result<Resource<PickerHandle>, ServiceError> {
        self.builder(self.table.get(&batch).map_err(|_| stale())?.invocation)?;
        self.require(Capability::Ui)?;
        if title.len() > MAX_UI_TITLE_BYTES {
            return Err(exhausted("plugin picker title is too long"));
        }
        let bytes = title.len();
        let handle = self
            .table
            .push(PickerHandle {
                invocation: self.invocation,
                action: self.response.actions.len(),
                finished: false,
                bytes,
                children: 0,
            })
            .map_err(|_| exhausted("plugin resource handle count exceeded"))?;
        if let Err(error) = self.action(
            Action::ShowUi {
                request,
                origin: target.map(origin),
                kind: UiKind::Picker {
                    title,
                    rows: Vec::new(),
                },
            },
            bytes,
        ) {
            let _ = self.table.delete(handle);
            return Err(error);
        }
        self.open_groups += 1;
        Ok(handle)
    }
    pub(super) fn ui_builtins(
        &mut self,
        batch: Resource<EffectsHandle>,
        request: u64,
        target: types::UiOrigin,
    ) -> Result<Resource<BuiltinHandle>, ServiceError> {
        self.builder(self.table.get(&batch).map_err(|_| stale())?.invocation)?;
        let handle = self
            .table
            .push(BuiltinHandle {
                invocation: self.invocation,
                action: self.response.actions.len(),
                finished: false,
            })
            .map_err(|_| exhausted("plugin resource handle count exceeded"))?;
        if let Err(error) = self.action(
            Action::InvokeBuiltin {
                request,
                origin: origin(target),
                commands: Vec::new(),
            },
            64,
        ) {
            let _ = self.table.delete(handle);
            return Err(error);
        }
        self.open_groups += 1;
        Ok(handle)
    }
    pub(super) fn ui_keymap(
        &mut self,
        batch: Resource<EffectsHandle>,
        request: u64,
    ) -> Result<Resource<KeymapHandle>, ServiceError> {
        self.builder(self.table.get(&batch).map_err(|_| stale())?.invocation)?;
        self.require(Capability::Ui)?;
        let handle = self
            .table
            .push(KeymapHandle {
                invocation: self.invocation,
                action: self.response.actions.len(),
                finished: false,
                children: 0,
            })
            .map_err(|_| exhausted("plugin resource handle count exceeded"))?;
        if let Err(error) = self.action(
            Action::UpdateKeymap {
                request,
                bindings: Vec::new(),
            },
            32,
        ) {
            let _ = self.table.delete(handle);
            return Err(error);
        }
        self.open_groups += 1;
        Ok(handle)
    }
}
impl host::HostPrompt for HostState {
    async fn initial(
        &mut self,
        handle: Resource<PromptHandle>,
        value: String,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let prompt = self.table.get(&handle).map_err(|_| stale())?;
            self.builder(prompt.invocation)?;
            if prompt.finished || prompt.initial_set {
                return Err(stale());
            }
            let action = prompt.action;
            if value.len() > MAX_UI_INPUT_BYTES {
                return Err(exhausted("plugin prompt input exceeds its size limit"));
            }
            self.reserve(value.len(), 0)?;
            self.table
                .get_mut(&handle)
                .map_err(|_| stale())?
                .initial_set = true;
            let Action::ShowUi {
                kind: UiKind::Prompt { initial, .. },
                ..
            } = &mut self.response.actions[action]
            else {
                unreachable!()
            };
            *initial = value;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn finish(
        &mut self,
        handle: Resource<PromptHandle>,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let prompt = self.table.get(&handle).map_err(|_| stale())?;
            self.builder(prompt.invocation)?;
            if prompt.finished {
                return Err(stale());
            }
            self.table.get_mut(&handle).map_err(|_| stale())?.finished = true;
            self.open_groups -= 1;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn drop(&mut self, handle: Resource<PromptHandle>) -> wasmtime::Result<()> {
        let prompt = self.table.delete(handle)?;
        if prompt.invocation == self.invocation && !prompt.finished {
            self.open_groups -= 1;
            self.rejected
                .get_or_insert_with(|| invalid("plugin dropped an unfinished prompt"));
        }
        Ok(())
    }
}
impl host::HostPicker for HostState {
    async fn row(
        &mut self,
        handle: Resource<PickerHandle>,
        id: String,
    ) -> wasmtime::Result<Result<Resource<PickerRowHandle>, types::Failure>> {
        let result = (|| {
            let picker = self.table.get(&handle).map_err(|_| stale())?;
            self.builder(picker.invocation)?;
            if picker.finished {
                return Err(stale());
            }
            let action = picker.action;
            if id.is_empty()
                || id.len() > 128
                || picker.bytes.saturating_add(id.len()) > MAX_UI_MODEL_BYTES
            {
                return Err(exhausted("plugin picker row exceeds its size limit"));
            }
            let Action::ShowUi {
                kind: UiKind::Picker { rows, .. },
                ..
            } = &self.response.actions[action]
            else {
                unreachable!()
            };
            if rows.len() >= MAX_UI_ROWS {
                return Err(exhausted("plugin picker row count exceeded"));
            }
            if rows.iter().any(|row| row.id == id) {
                return Err(invalid("plugin picker rows reuse an ID"));
            }
            let row = rows.len();
            self.reserve(id.len() + 64, 1)?;
            let child = self
                .table
                .push_child(
                    PickerRowHandle {
                        invocation: self.invocation,
                        action,
                        row,
                        parent: handle.rep(),
                        fields: 0,
                        finished: false,
                    },
                    &handle,
                )
                .map_err(|_| exhausted("plugin resource handle count exceeded"))?;
            let picker = self.table.get_mut(&handle).map_err(|_| stale())?;
            picker.bytes += id.len();
            picker.children += 1;
            let Action::ShowUi {
                kind: UiKind::Picker { rows, .. },
                ..
            } = &mut self.response.actions[action]
            else {
                unreachable!()
            };
            rows.push(UiRow {
                id,
                label: String::new(),
                description: String::new(),
                preview: None,
                location: None,
            });
            Ok(child)
        })();
        Ok(self.reject(result))
    }
    async fn finish(
        &mut self,
        handle: Resource<PickerHandle>,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let picker = self.table.get(&handle).map_err(|_| stale())?;
            self.builder(picker.invocation)?;
            if picker.finished || picker.children != 0 {
                return Err(invalid("plugin picker has unfinished rows"));
            }
            self.table.get_mut(&handle).map_err(|_| stale())?.finished = true;
            self.open_groups -= 1;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn drop(&mut self, handle: Resource<PickerHandle>) -> wasmtime::Result<()> {
        let picker = self.table.delete(handle)?;
        if picker.invocation == self.invocation && !picker.finished {
            self.open_groups -= 1;
            self.rejected
                .get_or_insert_with(|| invalid("plugin dropped an unfinished picker"));
        }
        Ok(())
    }
}
impl HostState {
    fn picker_field(
        &mut self,
        handle: Resource<PickerRowHandle>,
        value: String,
        field: u8,
        limit: usize,
    ) -> Result<(), ServiceError> {
        let row = self.table.get(&handle).map_err(|_| stale())?;
        self.builder(row.invocation)?;
        if row.finished || row.fields & field != 0 {
            return Err(stale());
        }
        let (action, index, parent) = (row.action, row.row, row.parent);
        let parent = Resource::<PickerHandle>::new_borrow(parent);
        let picker = self.table.get(&parent).map_err(|_| stale())?;
        if value.len() > limit || picker.bytes.saturating_add(value.len()) > MAX_UI_MODEL_BYTES {
            return Err(exhausted("plugin picker field exceeds its size limit"));
        }
        self.reserve(value.len(), 0)?;
        self.table.get_mut(&parent).map_err(|_| stale())?.bytes += value.len();
        self.table.get_mut(&handle).map_err(|_| stale())?.fields |= field;
        let Action::ShowUi {
            kind: UiKind::Picker { rows, .. },
            ..
        } = &mut self.response.actions[action]
        else {
            unreachable!()
        };
        match field {
            1 => rows[index].label = value,
            2 => rows[index].description = value,
            4 => rows[index].preview = Some(value),
            _ => unreachable!(),
        }
        Ok(())
    }
}
impl host::HostPickerRow for HostState {
    async fn label(
        &mut self,
        handle: Resource<PickerRowHandle>,
        value: String,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = self.picker_field(handle, value, 1, MAX_UI_ROW_BYTES);
        Ok(self.reject(result))
    }
    async fn description(
        &mut self,
        handle: Resource<PickerRowHandle>,
        value: String,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = self.picker_field(handle, value, 2, MAX_UI_ROW_BYTES);
        Ok(self.reject(result))
    }
    async fn preview(
        &mut self,
        handle: Resource<PickerRowHandle>,
        value: String,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = self.picker_field(handle, value, 4, MAX_UI_PREVIEW_BYTES);
        Ok(self.reject(result))
    }
    async fn location(
        &mut self,
        handle: Resource<PickerRowHandle>,
        value: types::UiLocation,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let row = self.table.get(&handle).map_err(|_| stale())?;
            self.builder(row.invocation)?;
            if row.finished || row.fields & 8 != 0 {
                return Err(stale());
            }
            let (action, index) = (row.action, row.row);
            let location = UiLocation {
                document: value.document,
                version: value.version,
                offset: value
                    .offset
                    .try_into()
                    .map_err(|_| invalid("picker offset exceeds host range"))?,
            };
            self.table.get_mut(&handle).map_err(|_| stale())?.fields |= 8;
            let Action::ShowUi {
                kind: UiKind::Picker { rows, .. },
                ..
            } = &mut self.response.actions[action]
            else {
                unreachable!()
            };
            rows[index].location = Some(location);
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn finish(
        &mut self,
        handle: Resource<PickerRowHandle>,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let row = self.table.get(&handle).map_err(|_| stale())?;
            self.builder(row.invocation)?;
            if row.finished {
                return Err(stale());
            }
            self.table.get_mut(&handle).map_err(|_| stale())?.finished = true;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn drop(&mut self, handle: Resource<PickerRowHandle>) -> wasmtime::Result<()> {
        let row = self.table.delete(handle)?;
        let parent = Resource::<PickerHandle>::new_borrow(row.parent);
        self.table.get_mut(&parent)?.children -= 1;
        if row.invocation == self.invocation && !row.finished {
            self.rejected
                .get_or_insert_with(|| invalid("plugin dropped an unfinished picker row"));
        }
        Ok(())
    }
}
impl host::HostBuiltinGroup for HostState {
    async fn add(
        &mut self,
        handle: Resource<BuiltinHandle>,
        command: types::BuiltinCommand,
        count: Option<u32>,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let group = self.table.get(&handle).map_err(|_| stale())?;
            self.builder(group.invocation)?;
            if group.finished {
                return Err(stale());
            }
            let action = group.action;
            let command = builtin(command);
            for capability in command.capabilities() {
                self.require(*capability)?;
            }
            let Action::InvokeBuiltin { commands, .. } = &self.response.actions[action] else {
                unreachable!()
            };
            if commands.len() >= MAX_BUILTIN_COMMANDS
                || count.is_some_and(|count| count == 0 || count as usize > MAX_BUILTIN_COUNT)
            {
                return Err(exhausted("plugin builtin composition exceeds its bounds"));
            }
            self.reserve(32, 1)?;
            let Action::InvokeBuiltin { commands, .. } = &mut self.response.actions[action] else {
                unreachable!()
            };
            commands.push(BuiltinInvocation {
                command,
                count: count.map(|count| count as usize),
            });
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn finish(
        &mut self,
        handle: Resource<BuiltinHandle>,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let group = self.table.get(&handle).map_err(|_| stale())?;
            self.builder(group.invocation)?;
            if group.finished {
                return Err(stale());
            }
            self.table.get_mut(&handle).map_err(|_| stale())?.finished = true;
            self.open_groups -= 1;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn drop(&mut self, handle: Resource<BuiltinHandle>) -> wasmtime::Result<()> {
        let group = self.table.delete(handle)?;
        if group.invocation == self.invocation && !group.finished {
            self.open_groups -= 1;
            self.rejected
                .get_or_insert_with(|| invalid("plugin dropped unfinished builtin commands"));
        }
        Ok(())
    }
}
impl host::HostKeymapGroup for HostState {
    async fn binding(
        &mut self,
        handle: Resource<KeymapHandle>,
        mode: types::KeymapMode,
        command: String,
    ) -> wasmtime::Result<Result<Resource<KeybindingHandle>, types::Failure>> {
        let result = (|| {
            let group = self.table.get(&handle).map_err(|_| stale())?;
            self.builder(group.invocation)?;
            if group.finished {
                return Err(stale());
            }
            let action = group.action;
            let Action::UpdateKeymap { bindings, .. } = &self.response.actions[action] else {
                unreachable!()
            };
            if bindings.len() >= MAX_PLUGIN_KEYBINDINGS || command.is_empty() || command.len() > 128
            {
                return Err(exhausted("plugin keymap binding exceeds its bounds"));
            }
            let binding = bindings.len();
            self.reserve(command.len() + 64, 1)?;
            let child = self
                .table
                .push_child(
                    KeybindingHandle {
                        invocation: self.invocation,
                        action,
                        binding,
                        parent: handle.rep(),
                        finished: false,
                    },
                    &handle,
                )
                .map_err(|_| exhausted("plugin resource handle count exceeded"))?;
            let mode = match mode {
                types::KeymapMode::Normal => KeymapMode::Normal,
                types::KeymapMode::Select => KeymapMode::Select,
                types::KeymapMode::Insert => KeymapMode::Insert,
            };
            let Action::UpdateKeymap { bindings, .. } = &mut self.response.actions[action] else {
                unreachable!()
            };
            bindings.push(PluginKeybinding {
                mode,
                command,
                keys: Vec::new(),
            });
            self.table.get_mut(&handle).map_err(|_| stale())?.children += 1;
            Ok(child)
        })();
        Ok(self.reject(result))
    }
    async fn finish(
        &mut self,
        handle: Resource<KeymapHandle>,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let group = self.table.get(&handle).map_err(|_| stale())?;
            self.builder(group.invocation)?;
            if group.finished || group.children != 0 {
                return Err(invalid("plugin keymap has unfinished bindings"));
            }
            self.table.get_mut(&handle).map_err(|_| stale())?.finished = true;
            self.open_groups -= 1;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn drop(&mut self, handle: Resource<KeymapHandle>) -> wasmtime::Result<()> {
        let group = self.table.delete(handle)?;
        if group.invocation == self.invocation && !group.finished {
            self.open_groups -= 1;
            self.rejected
                .get_or_insert_with(|| invalid("plugin dropped an unfinished keymap"));
        }
        Ok(())
    }
}
impl host::HostKeybinding for HostState {
    async fn key(
        &mut self,
        handle: Resource<KeybindingHandle>,
        value: String,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let group = self.table.get(&handle).map_err(|_| stale())?;
            self.builder(group.invocation)?;
            if group.finished {
                return Err(stale());
            }
            let (action, binding) = (group.action, group.binding);
            let Action::UpdateKeymap { bindings, .. } = &self.response.actions[action] else {
                unreachable!()
            };
            if bindings[binding].keys.len() >= 8 || value.is_empty() || value.len() > 64 {
                return Err(exhausted("plugin key path exceeds its bounds"));
            }
            self.reserve(value.len() + 24, 1)?;
            let Action::UpdateKeymap { bindings, .. } = &mut self.response.actions[action] else {
                unreachable!()
            };
            bindings[binding].keys.push(value);
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn finish(
        &mut self,
        handle: Resource<KeybindingHandle>,
    ) -> wasmtime::Result<Result<(), types::Failure>> {
        let result = (|| {
            let group = self.table.get(&handle).map_err(|_| stale())?;
            self.builder(group.invocation)?;
            if group.finished {
                return Err(stale());
            }
            let Action::UpdateKeymap { bindings, .. } = &self.response.actions[group.action] else {
                unreachable!()
            };
            if bindings[group.binding].keys.is_empty() {
                return Err(invalid("plugin key binding is empty"));
            }
            self.table.get_mut(&handle).map_err(|_| stale())?.finished = true;
            Ok(())
        })();
        Ok(self.reject(result))
    }
    async fn drop(&mut self, handle: Resource<KeybindingHandle>) -> wasmtime::Result<()> {
        let group = self.table.delete(handle)?;
        if group.invocation == self.invocation {
            let parent = Resource::<KeymapHandle>::new_borrow(group.parent);
            self.table.get_mut(&parent)?.children -= 1;
            if !group.finished {
                self.rejected
                    .get_or_insert_with(|| invalid("plugin dropped an unfinished key path"));
            }
        }
        Ok(())
    }
}
