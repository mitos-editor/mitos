//! Editor-owned autosave timing, insert-mode deferral, and save submission.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Weak,
    },
    time::Duration,
};

use arc_swap::access::Access;
use event::{register_hook, send_blocking, AsyncHook};
use tokio::{sync::mpsc::Sender, time::Instant};

use crate::{
    callbacks::EditorCallbackSender,
    document::Mode,
    events::{ConfigDidChange, DocumentDidChange},
    Editor,
};

struct State {
    callbacks: EditorCallbackSender,
    pending: AtomicBool,
    generation: AtomicU64,
}

pub struct AutoSaveHandler {
    state: Arc<State>,
    events: Sender<AutoSaveEvent>,
}

/// Documents can schedule work without keeping a closed editor's callback queue alive.
pub(crate) struct AutoSaveTrigger {
    state: Weak<State>,
    events: Sender<AutoSaveEvent>,
}

enum AutoSaveEvent {
    DocumentChanged { save_after: u64, generation: u64 },
    LeftInsertMode,
    Cancel,
}

impl AutoSaveHandler {
    pub fn new(callbacks: EditorCallbackSender) -> Self {
        let state = Arc::new(State {
            callbacks,
            pending: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        });
        let events = Debounce {
            state: Arc::downgrade(&state),
            generation: 0,
        }
        .spawn();
        Self { state, events }
    }

    pub(crate) fn trigger(&self) -> AutoSaveTrigger {
        AutoSaveTrigger {
            state: Arc::downgrade(&self.state),
            events: self.events.clone(),
        }
    }

    /// Forward a transition out of insert mode after updating the editor's mode.
    pub fn left_insert_mode(&self) {
        send_blocking(&self.events, AutoSaveEvent::LeftInsertMode);
    }

    fn cancel(&self) {
        self.state.generation.fetch_add(1, Ordering::Relaxed);
        self.state.pending.store(false, Ordering::Relaxed);
        send_blocking(&self.events, AutoSaveEvent::Cancel);
    }
}

impl AutoSaveTrigger {
    fn changed(&self, save_after: u64) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let generation = state.generation.fetch_add(1, Ordering::Relaxed) + 1;
        send_blocking(
            &self.events,
            AutoSaveEvent::DocumentChanged {
                save_after,
                generation,
            },
        );
    }
}

struct Debounce {
    state: Weak<State>,
    generation: u64,
}

impl AsyncHook for Debounce {
    type Event = AutoSaveEvent;

    fn handle_event(&mut self, event: Self::Event, timeout: Option<Instant>) -> Option<Instant> {
        match event {
            AutoSaveEvent::DocumentChanged {
                save_after,
                generation,
            } => {
                self.generation = generation;
                Some(Instant::now() + Duration::from_millis(save_after))
            }
            AutoSaveEvent::LeftInsertMode => {
                if timeout.is_none()
                    && self
                        .state
                        .upgrade()
                        .is_some_and(|state| state.pending.load(Ordering::Relaxed))
                {
                    self.finish_debounce();
                }
                // Leaving insert mode never shortens an active debounce.
                timeout
            }
            AutoSaveEvent::Cancel => None,
        }
    }

    fn finish_debounce(&mut self) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let owner = self.state.clone();
        let generation = self.generation;
        state.callbacks.send_blocking(move |editor| {
            let state = &editor.handlers.auto_save.state;
            if !owner.ptr_eq(&Arc::downgrade(state))
                || !editor.config().auto_save.after_delay.enable
                || state.generation.load(Ordering::Relaxed) != generation
            {
                return;
            }
            if editor.mode() == Mode::Insert {
                state.pending.store(true, Ordering::Relaxed);
            } else {
                // Clear before save preparation, which can itself emit document changes.
                state.pending.store(false, Ordering::Relaxed);
                request_auto_save(editor);
            }
        });
    }
}

fn request_auto_save(editor: &mut Editor) {
    if let Err(error) = crate::save::auto_save(editor) {
        editor.set_error(|| error.to_string());
    }
}

/// Focus-loss saves have their own setting and also run while in insert mode.
pub fn focus_lost(editor: &mut Editor) {
    if editor.config().auto_save.focus_lost {
        request_auto_save(editor);
    }
}

pub fn register_hooks() {
    event::runtime_local! { static REGISTER: std::sync::Once = std::sync::Once::new(); }
    REGISTER.call_once(|| {
        register_hook!(move |event: &mut DocumentDidChange<'_>| {
            let config = event.doc.config.load();
            if config.auto_save.after_delay.enable
                && let Some(trigger) = &event.doc.auto_save_trigger
            {
                trigger.changed(config.auto_save.after_delay.timeout);
            }
            Ok(())
        });
        register_hook!(move |event: &mut ConfigDidChange<'_>| {
            if !event.new.auto_save.after_delay.enable {
                event.editor.handlers.auto_save.cancel();
            }
            Ok(())
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_restart_the_delay_and_leaving_insert_mode_preserves_it() {
        let mut debounce = Debounce {
            state: Weak::new(),
            generation: 0,
        };
        let first = debounce.handle_event(
            AutoSaveEvent::DocumentChanged {
                save_after: 100,
                generation: 1,
            },
            None,
        );
        assert_eq!(
            debounce.handle_event(AutoSaveEvent::LeftInsertMode, first),
            first
        );
        let second = debounce.handle_event(
            AutoSaveEvent::DocumentChanged {
                save_after: 200,
                generation: 2,
            },
            first,
        );
        assert!(second > first);
        assert_eq!(
            debounce.handle_event(AutoSaveEvent::LeftInsertMode, second),
            second
        );
        assert_eq!(debounce.handle_event(AutoSaveEvent::Cancel, second), None);
        assert_eq!(
            debounce.handle_event(AutoSaveEvent::LeftInsertMode, None),
            None
        );
    }
}
