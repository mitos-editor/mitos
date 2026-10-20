use crate::{LinePos, TextRenderer};
use std::cmp::Ordering;
use text_layout::{doc_formatter::FormattedGrapheme, DisplayPosition};

/// Decorations are the primary mechanism for extending the text rendering.
///
/// Any on-screen element which is anchored to the rendered text in some form
/// should be implemented using this trait. Translating char positions to
/// on-screen positions can be expensive and should not be done manually in the
/// ui loop. Instead such translations are automatically performed on the fly
/// while the text is being rendered. The results are provided to this trait by
/// the rendering infrastructure.
///
/// To reserve space for virtual text lines (which is then filled by this trait) emit appropriate
/// [`LineAnnotation`](text_layout::text_annotations::LineAnnotation)s in the caller's text annotation preparation
pub trait Decoration {
    /// Called **before** a **visual** line is rendered. A visual line does not
    /// necessarily correspond to a single line in a document as soft wrapping can
    /// spread a single document line across multiple visual lines.
    ///
    /// This function is called before text is rendered as any decorations should
    /// never overlap the document text. That means that setting the foreground color
    /// here is (essentially) useless as the text color is overwritten by the
    /// rendered text. This _of course_ doesn't apply when rendering inside virtual lines
    /// below the line reserved by `LineAnnotation`s as no text will be rendered here.
    fn decorate_line(&mut self, _renderer: &mut TextRenderer, _pos: LinePos) {}

    /// Called **after** a **visual** line is rendered. A visual line does not
    /// necessarily correspond to a single line in a document as soft wrapping can
    /// spread a single document line across multiple visual lines.
    ///
    /// This function is called after text is rendered so that decorations can collect
    /// horizontal positions on the line (see [`Decoration::decorate_grapheme`]) first and
    /// use those positions while rendering virtual text. That means that setting the foreground
    /// color here is (essentially) useless as the text color is overwritten by the
    /// rendered text. This _of course_ doesn't apply when rendering inside virtual lines
    /// below the line reserved by `LineAnnotation`s.
    /// **Note**: To avoid overlapping decorations in the virtual lines, each decoration
    /// must return the number of virtual text lines it has taken up. Each `Decoration` receives
    /// an offset `virt_off` based on these return values where it can render virtual text:
    ///
    /// Return the additional display offset consumed by this decoration. The manager
    /// adds it to `virt_off` before invoking the next decoration on the same line.
    fn render_virt_lines(
        &mut self,
        _renderer: &mut TextRenderer,
        _pos: LinePos,
        _virt_off: DisplayPosition,
    ) -> DisplayPosition {
        DisplayPosition::new(0, 0)
    }

    fn reset_pos(&mut self, _pos: usize) -> usize {
        usize::MAX
    }

    fn skip_concealed_anchor(&mut self, conceal_end_char_idx: usize) -> usize {
        self.reset_pos(conceal_end_char_idx)
    }

    /// This function is called **before** the grapheme at `char_idx` is rendered.
    ///
    /// # Returns
    ///
    /// The character index of the next grapheme to visit. Returning the same
    /// index defers the anchor until a later grapheme, for example past inline
    /// virtual text to the document character at that position.
    fn decorate_grapheme(
        &mut self,
        _renderer: &mut TextRenderer,
        _grapheme: &FormattedGrapheme,
    ) -> usize {
        usize::MAX
    }
}

impl<F: FnMut(&mut TextRenderer, LinePos)> Decoration for F {
    fn decorate_line(&mut self, renderer: &mut TextRenderer, pos: LinePos) {
        self(renderer, pos);
    }
}

#[derive(Default)]
pub struct DecorationManager<'a> {
    decorations: Vec<(Box<dyn Decoration + 'a>, usize)>,
}

impl<'a> DecorationManager<'a> {
    pub fn add_decoration(&mut self, decoration: impl Decoration + 'a) {
        self.decorations.push((Box::new(decoration), 0));
    }

    pub(crate) fn prepare_for_rendering(&mut self, first_visible_char: usize) {
        for (decoration, next_position) in &mut self.decorations {
            *next_position = decoration.reset_pos(first_visible_char)
        }
    }

    pub(crate) fn decorate_grapheme(
        &mut self,
        renderer: &mut TextRenderer,
        grapheme: &FormattedGrapheme,
    ) {
        for (decoration, hook_char_idx) in &mut self.decorations {
            loop {
                match (*hook_char_idx).cmp(&grapheme.char_idx) {
                    // this grapheme has been concealed or we are at the first grapheme
                    Ordering::Less => {
                        *hook_char_idx = decoration.skip_concealed_anchor(grapheme.char_idx)
                    }
                    Ordering::Equal => {
                        *hook_char_idx = decoration.decorate_grapheme(renderer, grapheme);
                        break;
                    }
                    Ordering::Greater => break,
                }
            }
        }
    }

    pub(crate) fn decorate_line(&mut self, renderer: &mut TextRenderer, pos: LinePos) {
        for (decoration, _) in &mut self.decorations {
            decoration.decorate_line(renderer, pos);
        }
    }

    pub(crate) fn render_virtual_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        line_width: usize,
    ) {
        let mut virt_off = DisplayPosition::new(1, line_width); // start at 1 so the line is never overwritten
        for (decoration, _) in &mut self.decorations {
            virt_off += decoration.render_virt_lines(renderer, pos, virt_off);
        }
    }
}
