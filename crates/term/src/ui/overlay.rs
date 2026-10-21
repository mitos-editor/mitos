use editor_config::PickerLayout;
use tui::buffer::Buffer;
use view::{
    graphics::{Rect, RectExt as _},
    Editor,
};

use crate::compositor::{Component, Context, Cursor, Event, EventResult};

pub const FULL_OVERLAID_MAX_WIDTH: u16 = 200;

/// Places a component using the shared picker layout.
pub struct Overlay<T> {
    /// Child component
    pub content: T,
}

/// Place a picker according to the live layout preference, reserving two bottom rows.
pub fn overlaid<T>(content: T) -> Overlay<T> {
    Overlay { content }
}

fn picker_area(rect: Rect, layout: PickerLayout) -> Rect {
    let percentage = match layout {
        PickerLayout::Auto if rect.width < FULL_OVERLAID_MAX_WIDTH => 100,
        PickerLayout::Auto | PickerLayout::Centered => 90,
    };
    clip_rect_relative(rect.clip_bottom(2), percentage, percentage)
}

fn clip_rect_relative(rect: Rect, percent_horizontal: u8, percent_vertical: u8) -> Rect {
    fn mul_and_cast(size: u16, factor: u8) -> u16 {
        ((size as u32) * (factor as u32) / 100).try_into().unwrap()
    }

    let inner_w = mul_and_cast(rect.width, percent_horizontal);
    let inner_h = mul_and_cast(rect.height, percent_vertical);

    let offset_x = rect.width.saturating_sub(inner_w) / 2;
    let offset_y = rect.height.saturating_sub(inner_h) / 2;

    Rect {
        x: rect.x + offset_x,
        y: rect.y + offset_y,
        width: inner_w,
        height: inner_h,
    }
}

impl<T: Component + 'static> Component for Overlay<T> {
    #[cfg(feature = "integration")]
    fn has_pending_render(&mut self) -> bool {
        self.content.has_pending_render()
    }

    fn render(&mut self, area: Rect, frame: &mut Buffer, ctx: &mut Context) {
        let dimensions = picker_area(area, ctx.editor.config().picker.layout);
        // Resizing or a live layout change also changes the child's page size.
        self.content
            .required_size((dimensions.width, dimensions.height));
        self.content.render(dimensions, frame, ctx)
    }

    fn has_pending_keys(&self) -> bool {
        self.content.has_pending_keys()
    }

    fn handle_event(&mut self, event: &Event, ctx: &mut Context) -> EventResult {
        self.content.handle_event(event, ctx)
    }

    fn owns_cursor(&self) -> bool {
        self.content.owns_cursor()
    }

    fn cursor(&self, area: Rect, ctx: &Editor) -> Cursor {
        let dimensions = picker_area(area, ctx.config().picker.layout);
        self.content.cursor(dimensions, ctx)
    }

    fn id(&self) -> Option<&'static str> {
        self.content.id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_layout_switches_at_200_columns() {
        assert_eq!(
            picker_area(Rect::new(3, 5, 199, 42), PickerLayout::Auto),
            Rect::new(3, 5, 199, 40)
        );
        assert_eq!(
            picker_area(Rect::new(3, 5, 200, 42), PickerLayout::Auto),
            Rect::new(13, 7, 180, 36)
        );
    }

    #[test]
    fn centered_layout_keeps_margins_on_narrow_and_wide_terminals() {
        assert_eq!(
            picker_area(Rect::new(3, 5, 80, 42), PickerLayout::Centered),
            Rect::new(7, 7, 72, 36)
        );
        assert_eq!(
            picker_area(Rect::new(3, 5, 200, 42), PickerLayout::Centered),
            Rect::new(13, 7, 180, 36)
        );
    }

    #[test]
    fn layouts_stay_inside_tiny_areas_and_reserve_bottom_rows() {
        for layout in [PickerLayout::Auto, PickerLayout::Centered] {
            for width in [0, 1, 2, 80, 199, 200, u16::MAX - 3] {
                for height in [0, 1, 2, 3, 42, u16::MAX - 5] {
                    let area = Rect::new(3, 5, width, height);
                    let content = picker_area(area, layout);
                    assert!(content.x >= area.x && content.right() <= area.right());
                    assert!(content.y >= area.y);
                    assert!(content.bottom() <= area.bottom().saturating_sub(height.min(2)));
                }
            }
        }
    }
}
