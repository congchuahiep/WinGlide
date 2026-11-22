//! The settings card.
//!
//! `SettingItem::new(title)` is one card. With `.children([..])` the card shows
//! those items as rows below its own row; without it, the card is a single row.
//! Children are written inline, where they belong:
//!
//! ```ignore
//! SettingItem::new("Cycle taskbar buttons")
//!     .description("Switch windows based on taskbar buttons")
//!     .action(toggle)
//!     .children([
//!         SettingItem::note("By default, ..."),
//!         SettingItem::new("Cycle left").action(button),
//!     ])
//! ```
//!
//! # Struct or function?
//!
//! The settings UI uses three shapes, chosen by what the thing *is*:
//!
//! - **state or messages** -> a `Component` (`settings_window::Settings`);
//! - **a layout piece with named fields** -> a builder struct that converts into
//!   a [`View`] (this file). It is the equivalent of a props struct plus a render
//!   function, and it is what lets a card's children be written inline;
//! - **a leaf with no fields of its own** -> a free function returning [`View`]
//!   (`hotkey_button::hotkey_button`, `settings_window::toggle`).
//!
//! Children are an array (`[..]`): every child of a card is a `SettingItem`, so an
//! array is the one shape that covers any number of them, and `.children(..)`
//! takes any iterator anyway.

use windows_reactor::*;

/// One card of the settings window: a row, plus the rows inside it.
pub struct SettingItem {
    title: Option<String>,
    description: Option<String>,
    icon: Option<char>,
    action: Option<View>,
    children: Vec<SettingItem>,
    enabled: bool,
}

impl SettingItem {
    /// A card with a title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            description: None,
            icon: None,
            action: None,
            children: Vec::new(),
            enabled: true,
        }
    }

    /// A card that carries only a description - the note that opens a card body.
    pub fn note(description: impl Into<String>) -> Self {
        Self {
            title: None,
            description: Some(description.into()),
            icon: None,
            action: None,
            children: Vec::new(),
            enabled: true,
        }
    }

    /// Secondary text, drawn smaller and dimmer under the title.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Segoe Fluent Icons glyph shown in the left column.
    pub fn icon(mut self, glyph: char) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// Control on the right: a toggle, a hotkey button, a picker.
    pub fn action(mut self, action: impl Into<View>) -> Self {
        self.action = Some(action.into());
        self
    }

    /// Rows shown inside this card, top to bottom.
    pub fn children(mut self, children: impl IntoIterator<Item = SettingItem>) -> Self {
        self.children = children.into_iter().collect();
        self
    }

    /// Dims the row to half opacity, the state of a row whose feature is off.
    ///
    /// A child inherits its card's state, so `false` here dims the card and
    /// everything inside it.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

impl From<SettingItem> for View {
    fn from(item: SettingItem) -> Self {
        if item.children.is_empty() {
            return Border::new()
                .background(ThemeBrush::CardBackground)
                .border_brush(ThemeBrush::CardStroke)
                .border_thickness(Thickness::uniform(1.0))
                .corner_radius(CornerRadius::uniform(4.0))
                .content(row(&item, false, item.enabled));
        }

        // Every part of the card is its own surface, and the *bottom border of the
        // part above* is the rule under it. Drawing that rule as a rectangle over a
        // shared background - what this used to do - never matched: the seam under
        // the title and the lines between rows have to come from the same mechanism.
        let header = Border::new()
            .background(ThemeBrush::CardBackground)
            .border_brush(ThemeBrush::CardStroke)
            .border_thickness(Thickness::uniform(1.0))
            .corner_radius(CornerRadius::new(4.0, 4.0, 0.0, 0.0))
            .content(row(&item, false, item.enabled));

        // No top border on a child: the part above already drew that pixel. Keys
        // are positions, matching the positional identity the call site has.
        let last = item.children.len() - 1;
        let mut children = Vec::with_capacity(item.children.len());
        for (index, child) in item.children.iter().enumerate() {
            let enabled = item.enabled && child.enabled;
            let radius = if index == last {
                CornerRadius::new(0.0, 0.0, 4.0, 4.0)
            } else {
                CornerRadius::uniform(0.0)
            };
            children.push(KeyedView::new(
                index,
                Border::new()
                    .background(ThemeBrush::CardBackground)
                    .border_brush(ThemeBrush::CardStroke)
                    .border_thickness(Thickness::new(1.0, 0.0, 1.0, 1.0))
                    .corner_radius(radius)
                    .content(row(child, true, enabled)),
            ));
        }

        StackPanel::new()
            .children((header, StackPanel::new().keyed_children(children)))
            .into()
    }
}

/// One row: icon column, title and description, action on the right.
///
/// `is_child` selects the tighter child geometry - 54px tall, inset 38px on both
/// sides, action 4px from the edge - instead of a card's own 64px row.
fn row(item: &SettingItem, is_child: bool, enabled: bool) -> View {
    let icon_column = if item.icon.is_some() { 54.0 } else { 0.0 };
    let inset = if is_child { 38.0 } else { 0.0 };
    // Without an icon the text column starts where the icon column would end.
    let text_inset = if item.icon.is_some() { 0.0 } else { 16.0 };

    Grid::new()
        .columns([
            GridLength::Pixel(icon_column),
            GridLength::STAR,
            GridLength::Auto,
        ])
        .min_height(if is_child { 54.0 } else { 64.0 })
        .margin(Thickness::new(inset, 0.0, inset, 0.0))
        .horizontal_alignment(HorizontalAlignment::Stretch)
        .opacity(if enabled { 1.0 } else { 0.5 })
        .children((
            icon_cell(item.icon),
            text_cell(item, text_inset),
            action_cell(item.action.clone(), if is_child { 4.0 } else { 16.0 }),
        ))
}

fn icon_cell(icon: Option<char>) -> View {
    match icon {
        // WinUI 3's FontIcon already draws with Segoe Fluent Icons; 0.100 exposes
        // no font family or size override on it, so the theme size applies.
        Some(glyph) => FontIcon::new()
            .glyph(glyph.to_string())
            .horizontal_alignment(HorizontalAlignment::Center)
            .vertical_alignment(VerticalAlignment::Center)
            .grid_column(0)
            .into(),
        None => placeholder(),
    }
}

fn text_cell(item: &SettingItem, left: f64) -> View {
    Border::new()
        .padding(Thickness::xy(0.0, 8.0))
        .margin(Thickness::new(left, 0.0, 0.0, 0.0))
        .vertical_alignment(VerticalAlignment::Center)
        .grid_column(1)
        .content(text_stack(item))
}

/// Title and description, with only the lines that actually exist.
///
/// Keeping an always-present empty `TextBlock` for the missing line still cost the
/// stack's 2px gap, which showed up as stray padding above every note.
fn text_stack(item: &SettingItem) -> View {
    match (&item.title, &item.description) {
        (Some(title), Some(description)) => StackPanel::new()
            .spacing(2.0)
            .children((title_text(title), description_text(description)))
            .into(),
        (Some(title), None) => title_text(title).into(),
        (None, Some(description)) => description_text(description).into(),
        (None, None) => placeholder(),
    }
}

fn title_text(title: &str) -> TextBlock {
    TextBlock::new().text(title).font_size(14.0)
}

fn description_text(description: &str) -> TextBlock {
    TextBlock::new()
        .text(description)
        .font_size(12.0)
        .foreground(ThemeBrush::PrimaryText)
        .opacity(0.62)
        .text_wrapping(TextWrapping::Wrap)
}

fn action_cell(action: Option<View>, right: f64) -> View {
    Grid::new()
        .vertical_alignment(VerticalAlignment::Center)
        .margin(Thickness::new(0.0, 0.0, right, 0.0))
        .grid_column(2)
        .children((action.unwrap_or_else(placeholder),))
}

/// An invisible zero-size control that keeps a fixed row shape where a cell has
/// nothing to show.
fn placeholder() -> View {
    Border::new().width(0.0).height(0.0).into()
}
