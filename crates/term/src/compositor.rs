// Each component declares its own size constraints and gets fitted based on its parent.
// Q: how does this work with popups?
// cursive does compositor.screen_mut().add_layer_at(pos::absolute(x, y), <component>)
pub use tui::terminal::Cursor;
use view::graphics::Rect;

use tui::buffer::Buffer as Surface;

pub type Callback = Box<dyn FnOnce(&mut Compositor, &mut Context)>;

// Cursive-inspired
pub enum EventResult {
    Ignored(Option<Callback>),
    Consumed(Option<Callback>),
}

use crate::job::Jobs;
use crate::ui::{picker, prompt::PromptSession, Prompt};
use std::sync::{Arc, Mutex};
use view::Editor;

pub use ui_core::input::Event;

/// Terminal input continuations belong to their application, not the shared editor.
#[derive(Default)]
pub struct InteractionState {
    /// Whether the current application layout offers the sidebar as a focus target.
    pub sidebar_visible: bool,
    pub count: Option<std::num::NonZeroUsize>,
    pub selected_register: Option<char>,
    pub macro_recording: Option<(char, Vec<ui_core::input::KeyEvent>)>,
    pub macro_replaying: Vec<char>,
    pub mouse_down_range: Option<editor_core::Range>,
}

/// Input held until a component's asynchronous action establishes its target.
pub(crate) struct DeferredEvent {
    event: Event,
    playback: Vec<char>,
}

impl DeferredEvent {
    pub(crate) fn capture(event: &Event, interaction: &mut InteractionState) -> Self {
        // Record deferred keys when they reach their eventual input target.
        if interaction.macro_replaying.is_empty()
            && matches!(event, Event::Key(_))
            && let Some((_, keys)) = &mut interaction.macro_recording
        {
            keys.pop();
        }
        Self {
            event: event.clone(),
            playback: interaction.macro_replaying.clone(),
        }
    }
}

pub struct Context<'a> {
    pub config: crate::config::Context<'a>,
    pub editor: &'a mut Editor,
    pub interaction: &'a mut InteractionState,
    pub scroll: Option<usize>,
    pub jobs: &'a mut Jobs,
    pub image_picker: Option<&'a ratatui_image::picker::Picker>,
    /// Whether the component being rendered owns the active cursor this frame.
    pub is_cursor_owner: bool,
}

/// Drawing reads editor state; background work belongs to Component::update.
pub struct DrawContext<'a> {
    pub hyperlinks: std::cell::RefCell<ui_core::hyperlinks::HyperlinkBuffer>,
    pub config: &'a crate::config::Config,
    pub editor: &'a Editor,
    pub interaction: &'a InteractionState,
    /// Absolute document cursor produced by this frame's focused pane.
    pub editor_cursor: Option<text_layout::DisplayPosition>,
    pub scroll: Option<usize>,
    pub is_cursor_owner: bool,
}

impl Context<'_> {
    pub fn draw_context(&self) -> DrawContext<'_> {
        DrawContext {
            hyperlinks: Default::default(),
            config: self.config.current,
            editor: self.editor,
            interaction: self.interaction,
            editor_cursor: self.editor.cursor().0,
            scroll: self.scroll,
            is_cursor_owner: self.is_cursor_owner,
        }
    }

    pub fn after_writes(
        &mut self,
        action: impl FnOnce(&mut Editor) -> anyhow::Result<()> + Send + 'static,
    ) -> anyhow::Result<()> {
        self.jobs.after_writes(self.editor, Box::new(action))
    }
}

pub trait Component: Any + AnyComponent {
    /// Active input contexts in dispatch order, for read-only binding inspection.
    fn keymap_contexts(&self, _cx: &Context) -> Vec<crate::keymap::inspect::Context> {
        Vec::new()
    }

    /// Prepare state, apply ready results and schedule work before drawing.
    fn update(&mut self, _area: Rect, _cx: &mut Context) {}

    /// Run both phases when presenting a standalone component.
    fn update_and_render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        self.update(area, cx);
        self.render(area, surface, &mut cx.draw_context());
    }

    /// Process input events, return true if handled.
    fn handle_event(&mut self, _event: &Event, _ctx: &mut Context) -> EventResult {
        EventResult::Ignored(None)
    }
    /// Whether this component or a child owns the continuation of a key sequence.
    fn has_pending_keys(&self) -> bool {
        false
    }

    /// Matching work whose results must render before an integration assertion.
    /// Pending external providers do not prevent the editor from becoming idle.
    fn has_pending_render(&self) -> bool {
        false
    }

    /// Render the component onto the provided surface.
    fn render(&mut self, area: Rect, frame: &mut Surface, ctx: &mut DrawContext);

    /// Claim the active cursor, even when it is offscreen or hidden. The topmost
    /// claimant wins; decorative layers leave ownership with the layer below.
    fn owns_cursor(&self) -> bool {
        false
    }

    /// Resolve the owned cursor after rendering has established its position.
    fn cursor(&self, _area: Rect, _cx: &DrawContext) -> Cursor {
        Cursor::Hidden
    }

    /// May be used by the parent component to compute the child area.
    /// viewport is the maximum allowed area, and the child should stay within those bounds.
    ///
    /// The returned size might be larger than the viewport if the child is too big to fit.
    /// In this case the parent can use the values to calculate scroll.
    fn required_size(&mut self, _viewport: (u16, u16)) -> Option<(u16, u16)> {
        None
    }

    fn id(&self) -> Option<&'static str> {
        None
    }
}

/// Prompt identities visible to editor hooks without retaining UI components.
#[derive(Clone, Default)]
pub(crate) struct PromptSessions(Arc<Mutex<Vec<PromptSession>>>);

impl PromptSessions {
    pub(crate) fn snapshot(&self) -> Vec<PromptSession> {
        self.0.lock().unwrap().clone()
    }
}

pub struct Compositor {
    prompt_sessions: PromptSessions,
    layers: Vec<Box<dyn Component>>,
    area: Rect,

    pub(crate) last_picker: Option<Box<dyn Component>>,
    pub(crate) full_redraw: bool,
}

impl Compositor {
    pub(crate) fn replay_deferred(&mut self, input: Vec<DeferredEvent>, cx: &mut Context) {
        for DeferredEvent { event, playback } in input {
            if cx.editor.should_close() {
                break;
            }
            let previous = std::mem::replace(&mut cx.interaction.macro_replaying, playback);
            if cx.interaction.macro_replaying.is_empty() {
                self.handle_event(&event, cx);
            } else {
                self.replay_event(&event, cx);
            }
            cx.interaction.macro_replaying = previous;
        }
    }

    pub(crate) fn keymap_contexts(&self, cx: &Context) -> Vec<crate::keymap::inspect::Context> {
        self.layers
            .iter()
            .rev()
            .flat_map(|layer| layer.keymap_contexts(cx))
            .collect()
    }

    pub(crate) fn has_pending_render(&self) -> bool {
        self.layers.iter().any(|layer| layer.has_pending_render())
    }

    pub fn new(area: Rect) -> Self {
        Self {
            layers: Vec::new(),
            prompt_sessions: PromptSessions::default(),
            area,
            last_picker: None,
            full_redraw: false,
        }
    }

    pub fn size(&self) -> Rect {
        self.area
    }

    pub fn resize(&mut self, area: Rect) {
        self.area = area;
    }

    /// Add a layer to be rendered in front of all existing layers.
    pub fn push(&mut self, mut layer: Box<dyn Component>) {
        // immediately clear last_picker field to avoid excessive memory
        // consumption for picker with many items
        if layer.id() == Some(picker::ID) {
            self.last_picker = None;
        }
        let size = self.size();
        // trigger required_size on init
        layer.required_size((size.width, size.height));
        self.layers.push(layer);
        self.refresh_prompt_sessions();
    }

    /// Replace a component that has the given `id` with the new layer and if
    /// no component is found, push the layer normally.
    pub fn replace_or_push<T: Component>(&mut self, id: &'static str, layer: T) {
        if let Some(component) = self.find_id(id) {
            *component = layer;
            self.refresh_prompt_sessions();
        } else {
            self.push(Box::new(layer))
        }
    }

    pub fn pop(&mut self) -> Option<Box<dyn Component>> {
        let layer = self.layers.pop();
        self.refresh_prompt_sessions();
        layer
    }

    pub fn remove(&mut self, id: &'static str) -> Option<Box<dyn Component>> {
        let idx = self
            .layers
            .iter()
            .position(|layer| layer.id() == Some(id))?;
        let layer = self.layers.remove(idx);
        self.refresh_prompt_sessions();
        Some(layer)
    }

    pub(crate) fn prompt_sessions(&self) -> PromptSessions {
        self.prompt_sessions.clone()
    }

    fn refresh_prompt_sessions(&self) {
        *self.prompt_sessions.0.lock().unwrap() = self
            .layers
            .iter()
            .filter_map(|layer| layer.as_any().downcast_ref::<Prompt>())
            .map(|prompt| prompt.session().clone())
            .collect();
    }

    pub(crate) fn remove_prompt_sessions(&mut self, sessions: &[PromptSession]) {
        self.layers.retain(|layer| {
            layer
                .as_any()
                .downcast_ref::<Prompt>()
                .is_none_or(|prompt| {
                    !sessions
                        .iter()
                        .any(|session| session.same(prompt.session()))
                })
        });
        self.refresh_prompt_sessions();
    }

    pub fn handle_event(&mut self, event: &Event, cx: &mut Context) -> bool {
        // If it is a key event, a macro is being recorded, and a macro isn't being replayed,
        // push the key event to the recording.
        if let (Event::Key(key), Some((_, keys))) = (event, &mut cx.interaction.macro_recording)
            && cx.interaction.macro_replaying.is_empty()
        {
            keys.push(*key);
        }

        self.replay_event(event, cx)
    }

    /// Dispatch input that has already passed through macro playback.
    pub(crate) fn replay_event(&mut self, event: &Event, cx: &mut Context) -> bool {
        let mut callbacks = Vec::new();
        let mut consumed = false;

        // propagate events through the layers until we either find a layer that consumes it or we
        // run out of layers (event bubbling), starting at the front layer and then moving to the
        // background.
        for layer in self.layers.iter_mut().rev() {
            match layer.handle_event(event, cx) {
                EventResult::Consumed(Some(callback)) => {
                    callbacks.push(callback);
                    consumed = true;
                    break;
                }
                EventResult::Consumed(None) => {
                    consumed = true;
                    break;
                }
                EventResult::Ignored(Some(callback)) => {
                    callbacks.push(callback);
                }
                EventResult::Ignored(None) => {}
            };
        }

        for callback in callbacks {
            callback(self, cx)
        }

        consumed
    }

    pub fn update(&mut self, area: Rect, cx: &mut Context) {
        for layer in &mut self.layers {
            layer.update(area, cx);
        }
    }

    pub fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut DrawContext) -> Cursor {
        *cx.hyperlinks.get_mut() = ui_core::hyperlinks::HyperlinkBuffer::new(area);
        let owner = self.layers.iter().rposition(|layer| layer.owns_cursor());
        for (index, layer) in self.layers.iter_mut().enumerate() {
            cx.is_cursor_owner = owner == Some(index);
            layer.render(area, surface, cx);
        }
        cx.is_cursor_owner = false;
        owner.map_or(Cursor::Hidden, |index| self.layers[index].cursor(area, cx))
    }

    pub fn has_component<T: Any>(&self) -> bool {
        self.layers
            .iter()
            .any(|component| component.as_any().is::<T>())
    }

    pub(crate) fn find_prompt(&mut self, session: &PromptSession) -> Option<&mut Prompt> {
        self.layers.iter_mut().find_map(|layer| {
            layer
                .as_any_mut()
                .downcast_mut::<Prompt>()
                .filter(|prompt| prompt.session().same(session))
        })
    }

    pub fn find<T: 'static>(&mut self) -> Option<&mut T> {
        self.layers
            .iter_mut()
            .find_map(|component| component.as_any_mut().downcast_mut())
    }

    pub fn find_id<T: 'static>(&mut self, id: &'static str) -> Option<&mut T> {
        self.layers
            .iter_mut()
            .find(|component| component.id() == Some(id))
            .and_then(|component| component.as_any_mut().downcast_mut())
    }

    pub fn need_full_redraw(&mut self) {
        self.full_redraw = true;
    }

    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
}

// View casting, taken straight from Cursive

use std::any::Any;

/// A view that can be downcasted to its concrete type.
///
/// This trait is automatically implemented for any `T: Component`.
pub trait AnyComponent {
    /// Downcast self to a `Any`.
    fn as_any(&self) -> &dyn Any;

    /// Downcast self to a mutable `Any`.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Returns a boxed any from a boxed self.
    ///
    /// Can be used before `Box::downcast()`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use term::{ui::Text, compositor::Component};
    /// let boxed: Box<dyn Component> = Box::new(Text::new("text".to_string()));
    /// let text: Box<Text> = boxed.as_boxed_any().downcast().unwrap();
    /// ```
    fn as_boxed_any(self: Box<Self>) -> Box<dyn Any>;
}

impl<T: Component> AnyComponent for T {
    /// Downcast self to a `Any`.
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// Downcast self to a mutable `Any`.
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_boxed_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl dyn AnyComponent {
    /// Attempts to downcast `self` to a concrete type.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.as_any().downcast_ref()
    }

    /// Attempts to downcast `self` to a concrete type.
    pub fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.as_any_mut().downcast_mut()
    }

    /// Attempts to downcast `Box<Self>` to a concrete type.
    pub fn downcast<T: Any>(self: Box<Self>) -> Result<Box<T>, Box<Self>> {
        // Do the check here + unwrap, so the error
        // value is `Self` and not `dyn Any`.
        if self.as_any().is::<T>() {
            Ok(self.as_boxed_any().downcast().unwrap())
        } else {
            Err(self)
        }
    }

    /// Checks if this view is of type `T`.
    pub fn is<T: Any>(&mut self) -> bool {
        self.as_any().is::<T>()
    }
}
