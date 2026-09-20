//! Single-line text input for one editable block.
//!
//! Adapted from GPUI's `input` example: keyboard editing, mouse selection,
//! IME support, custom-rendered cursor. Edits are reported to the workspace
//! as [`BlockEvent`]s so the block model stays the source of truth.

use std::ops::Range;

use gpui::{
    div, fill, hsla, point, prelude::*, px, relative, rgba, App, Bounds, ClipboardItem, Context,
    CursorStyle, ElementId, ElementInputHandler, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, Window,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::actions::{
    Backspace, Copy, Cut, Delete, End, Home, Left, Paste, Right, SelectAll, SelectLeft,
    SelectRight, SplitBlock,
};

fn is_keyword(lang: Option<&str>, word: &str) -> bool {
    let lang = lang.unwrap_or("").to_ascii_lowercase();
    match lang.as_str() {
        "python" | "py" => matches!(
            word,
            "and"
                | "as"
                | "assert"
                | "break"
                | "class"
                | "continue"
                | "def"
                | "del"
                | "elif"
                | "else"
                | "except"
                | "finally"
                | "for"
                | "from"
                | "global"
                | "if"
                | "import"
                | "in"
                | "is"
                | "lambda"
                | "nonlocal"
                | "not"
                | "or"
                | "pass"
                | "raise"
                | "return"
                | "try"
                | "while"
                | "with"
                | "yield"
                | "True"
                | "False"
                | "None"
        ),
        "javascript" | "js" | "typescript" | "ts" => matches!(
            word,
            "break"
                | "case"
                | "catch"
                | "class"
                | "const"
                | "continue"
                | "debugger"
                | "default"
                | "delete"
                | "do"
                | "else"
                | "export"
                | "extends"
                | "finally"
                | "for"
                | "function"
                | "if"
                | "import"
                | "in"
                | "instanceof"
                | "let"
                | "new"
                | "return"
                | "super"
                | "switch"
                | "this"
                | "throw"
                | "try"
                | "var"
                | "while"
                | "with"
                | "yield"
                | "of"
                | "from"
                | "async"
                | "await"
        ),
        "rust" | "rs" => matches!(
            word,
            "as" | "break"
                | "const"
                | "continue"
                | "crate"
                | "else"
                | "enum"
                | "extern"
                | "false"
                | "fn"
                | "for"
                | "if"
                | "impl"
                | "in"
                | "let"
                | "loop"
                | "match"
                | "mod"
                | "move"
                | "mut"
                | "pub"
                | "ref"
                | "return"
                | "self"
                | "Self"
                | "static"
                | "struct"
                | "super"
                | "trait"
                | "true"
                | "type"
                | "unsafe"
                | "use"
                | "where"
                | "while"
                | "async"
                | "await"
                | "dyn"
        ),
        _ => matches!(
            word,
            "for" | "if" | "else" | "while" | "return" | "import" | "from" | "in"
        ),
    }
}

fn highlight_runs_for_line(
    line: &str,
    base_font: gpui::Font,
    base_color: gpui::Hsla,
    keyword_color: gpui::Hsla,
    lang: Option<&str>,
) -> Vec<gpui::TextRun> {
    if line.is_empty() {
        return vec![gpui::TextRun {
            len: 0,
            font: base_font,
            color: base_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
    }
    let mut runs: Vec<gpui::TextRun> = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let remaining = &line[i..];
        let first = remaining.chars().next().unwrap();
        let is_word_char = first.is_alphanumeric() || first == '_';
        let mut j = i + first.len_utf8();
        if is_word_char {
            while j < line.len() {
                let c = line[j..].chars().next().unwrap();
                if c.is_alphanumeric() || c == '_' {
                    j += c.len_utf8();
                } else {
                    break;
                }
            }
            let word = &line[i..j];
            let is_kw = is_keyword(lang, word);
            let color = if is_kw { keyword_color } else { base_color };
            // Merge with previous if same color
            if let Some(last) = runs.last_mut() {
                if last.color == color && last.font == base_font {
                    last.len += word.len();
                } else {
                    runs.push(gpui::TextRun {
                        len: word.len(),
                        font: base_font.clone(),
                        color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    });
                }
            } else {
                runs.push(gpui::TextRun {
                    len: word.len(),
                    font: base_font.clone(),
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                });
            }
            i = j;
        } else {
            // Consume consecutive non-word chars as one run
            let mut j = i + first.len_utf8();
            while j < line.len() {
                let c = line[j..].chars().next().unwrap();
                if c.is_alphanumeric() || c == '_' {
                    break;
                }
                j += c.len_utf8();
            }
            let len = j - i;
            let color = base_color;
            if let Some(last) = runs.last_mut() {
                if last.color == color {
                    last.len += len;
                } else {
                    runs.push(gpui::TextRun {
                        len,
                        font: base_font.clone(),
                        color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    });
                }
            } else {
                runs.push(gpui::TextRun {
                    len,
                    font: base_font.clone(),
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                });
            }
            i = j;
        }
    }
    // Clean zero-len runs
    runs.retain(|r| r.len > 0);
    if runs.is_empty() {
        runs.push(gpui::TextRun {
            len: line.len(),
            font: base_font,
            color: base_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    runs
}

/// Events a block input sends to the workspace view.
#[derive(Clone, Debug)]
pub enum BlockEvent {
    /// Text changed (keystroke, paste, IME, ...).
    Changed {
        block_idx: usize,
        item_idx: Option<usize>,
        text: String,
    },
    /// Enter pressed: split at cursor.
    Split {
        block_idx: usize,
        item_idx: Option<usize>,
        cursor: usize,
    },
    /// Backspace on an empty block: delete it.
    DeleteEmpty {
        block_idx: usize,
        item_idx: Option<usize>,
    },
}

pub struct BlockInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: Option<SharedString>,
    block_idx: usize,
    item_idx: Option<usize>,
    multiline: bool,
    language: Option<String>,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    // For multiline (code) we store all shaped lines; otherwise single.
    last_lines: Option<Vec<ShapedLine>>,
}

impl BlockInput {
    pub fn new(block_idx: usize, content: String, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: content.into(),
            placeholder: None,
            block_idx,
            item_idx: None,
            multiline: false,
            language: None,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            last_lines: None,
        }
    }

    /// Create a block input for a list item (bulleted/numbered/tasks).
    pub fn new_item(
        block_idx: usize,
        item_idx: usize,
        content: String,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: content.into(),
            placeholder: None,
            block_idx,
            item_idx: Some(item_idx),
            multiline: false,
            language: None,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            last_lines: None,
        }
    }

    /// Mark this input as multiline (used for code blocks so Enter inserts a newline
    /// and the block grows with its content).
    pub fn multiline(mut self, multiline: bool) -> Self {
        self.multiline = multiline;
        self
    }

    pub fn with_language(mut self, lang: Option<String>) -> Self {
        self.language = lang;
        self
    }

    /// Placeholder hint shown when the block is empty (Notion-style).
    pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Move the cursor (used when focusing after split/delete).
    pub fn set_cursor(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());
        let offset = self.content.floor_char_boundary(offset);
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn emit_changed(&self, cx: &mut Context<Self>) {
        cx.emit(BlockEvent::Changed {
            block_idx: self.block_idx,
            item_idx: self.item_idx,
            text: self.content.to_string(),
        });
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn split(&mut self, _: &SplitBlock, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            // Code blocks: Enter inserts a newline with auto-indent.
            // Carry the current line's leading whitespace, and add an extra level
            // if the line ends with ':' (Python) or '{'/'(' (brace languages).
            let cursor = self.cursor_offset();
            let content = self.content.to_string();
            let line_start = content[..cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
            let line_end = content[line_start..]
                .find('\n')
                .map(|p| line_start + p)
                .unwrap_or(content.len());
            let line = &content[line_start..line_end];
            let indent: String = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let trimmed = line.trim_end();
            let lang = self.language.as_deref().map(|s| s.to_ascii_lowercase());
            let is_py = lang.as_deref().is_some_and(|l| l == "python" || l == "py");
            let needs_extra = if is_py {
                trimmed.ends_with(':')
            } else {
                trimmed.ends_with(':') || trimmed.ends_with('{') || trimmed.ends_with('(')
            };
            let extra = if needs_extra { "    " } else { "" };
            let insert = format!("\n{}{}", indent, extra);
            self.replace_text_in_range(None, &insert, window, cx);
            return;
        }
        cx.emit(BlockEvent::Split {
            block_idx: self.block_idx,
            item_idx: self.item_idx,
            cursor: self.cursor_offset(),
        });
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.content.is_empty() && self.cursor_offset() == 0 {
            cx.emit(BlockEvent::DeleteEmpty {
                block_idx: self.block_idx,
                item_idx: self.item_idx,
            });
            return;
        }
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let sanitized = if self.multiline {
                text
            } else {
                text.replace('\n', " ")
            };
            self.replace_text_in_range(None, &sanitized, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        if self.multiline {
            if let Some(lines) = self.last_lines.as_ref() {
                let h = if lines.is_empty() {
                    // fallback
                    return 0;
                } else {
                    (bounds.bottom() - bounds.top()) / lines.len() as f32
                };
                let y_off = position.y - bounds.top();
                let line_idx = (y_off / h).floor() as usize;
                let line_idx = line_idx.min(lines.len() - 1);
                let line = &lines[line_idx];
                let x = line.closest_index_for_x(position.x - bounds.left());
                // Convert x (byte offset within line) to global byte offset
                let mut off = 0usize;
                for (i, l) in lines.iter().enumerate() {
                    if i == line_idx {
                        return off + x;
                    }
                    off += l.text.len() + 1; // +1 for '\n'
                }
                return off;
            }
        }
        let Some(line) = self.last_layout.as_ref() else {
            return 0;
        };
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EventEmitter<BlockEvent> for BlockInput {}

impl EntityInputHandler for BlockInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        Some(gpui::UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
        self.emit_changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        cx.notify();
        self.emit_changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: gpui::Entity<BlockInput>,
}

struct PrepaintState {
    lines: Vec<ShapedLine>,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
    // Back-compat for single-line path (kept for bounds calc)
    line: Option<ShapedLine>,
    selection: Option<PaintQuad>,
}

fn display_empty(s: &str) -> bool {
    s.is_empty()
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl gpui::Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let input = self.input.read(cx);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if input.multiline {
            let content = input.content.clone();
            let is_placeholder = display_empty(&content) && input.placeholder.is_some();
            let text_for_count: SharedString = if is_placeholder {
                input.placeholder.clone().unwrap()
            } else {
                content.clone()
            };
            // Count physical lines (split by '\n'); at least 1.
            let line_count = text_for_count.split('\n').count().max(1);
            let h = window.line_height() * line_count as f32;
            style.size.height = h.into();
        } else {
            style.size.height = window.line_height().into();
        }
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();
        let is_multiline = input.multiline;

        if is_multiline {
            let is_placeholder = display_empty(&content) && input.placeholder.is_some();
            let raw_text: SharedString = if is_placeholder {
                input.placeholder.clone().unwrap()
            } else {
                content.clone()
            };
            let lines_text: Vec<SharedString> = if is_placeholder {
                vec![raw_text.clone()]
            } else {
                raw_text
                    .split('\n')
                    .map(|s| SharedString::from(s.to_string()))
                    .collect()
            };
            let line_height = window.line_height();
            let font_size = style.font_size.to_pixels(window.rem_size());
            let lang = input.language.clone();
            let keyword_color = hsla(0.79, 0.62, 0.68, 1.0); // muted purple for keywords
            let mut shaped: Vec<ShapedLine> = Vec::with_capacity(lines_text.len());
            for lt in &lines_text {
                if is_placeholder {
                    let run = gpui::TextRun {
                        len: lt.len(),
                        font: style.font(),
                        color: style.color.opacity(0.45),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window
                        .text_system()
                        .shape_line(lt.clone(), font_size, &[run], None);
                    shaped.push(line);
                } else if input.multiline && lang.is_some() {
                    let runs = highlight_runs_for_line(
                        lt.as_ref(),
                        style.font(),
                        style.color,
                        keyword_color,
                        lang.as_deref(),
                    );
                    let line = window
                        .text_system()
                        .shape_line(lt.clone(), font_size, &runs, None);
                    shaped.push(line);
                } else {
                    let run = gpui::TextRun {
                        len: lt.len(),
                        font: style.font(),
                        color: style.color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window
                        .text_system()
                        .shape_line(lt.clone(), font_size, &[run], None);
                    shaped.push(line);
                }
            }
            // Cursor: find which physical line the byte offset lies on.
            let mut byte_offset = 0usize;
            let mut cursor_line = 0usize;
            let mut cursor_in_line = cursor;
            for (idx, lt) in lines_text.iter().enumerate() {
                let line_len = lt.len();
                let line_end = byte_offset + line_len;
                // Cursor at line_end is considered start of next line if not last.
                if cursor <= line_end || idx == lines_text.len() - 1 {
                    cursor_line = idx;
                    cursor_in_line = cursor.saturating_sub(byte_offset).min(line_len);
                    break;
                }
                byte_offset = line_end + 1; // skip '\n'
            }
            let cursor_x = shaped[cursor_line].x_for_index(cursor_in_line);
            let cursor_y = bounds.top() + line_height * cursor_line as f32;
            let cursor_quad = if selected_range.is_empty() && !is_placeholder {
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_x, cursor_y),
                        gpui::size(px(2.), line_height),
                    ),
                    gpui::blue(),
                ))
            } else {
                None
            };
            // Selections: one quad per intersecting line.
            let mut selections = Vec::new();
            if !selected_range.is_empty() && !is_placeholder {
                let mut off = 0usize;
                for (idx, lt) in lines_text.iter().enumerate() {
                    let line_start = off;
                    let line_end = off + lt.len();
                    let sel_start = selected_range.start.max(line_start);
                    let sel_end = selected_range.end.min(line_end);
                    if sel_start < sel_end {
                        let s = sel_start - line_start;
                        let e = sel_end - line_start;
                        let x0 = shaped[idx].x_for_index(s);
                        let x1 = shaped[idx].x_for_index(e);
                        let y = bounds.top() + line_height * idx as f32;
                        selections.push(fill(
                            Bounds::from_corners(
                                point(bounds.left() + x0, y),
                                point(bounds.left() + x1, y + line_height),
                            ),
                            rgba(0x3311ff30),
                        ));
                    }
                    // Also handle selection that includes the newline itself (paint empty)
                    off = line_end + 1;
                }
            }
            return PrepaintState {
                lines: shaped,
                cursor: cursor_quad,
                selections,
                line: None,
                selection: None,
            };
        }

        // Single-line element: render soft breaks as spaces (content keeps them).
        let is_placeholder = display_empty(&content) && input.placeholder.is_some();
        let display_text: SharedString = if is_placeholder {
            input.placeholder.clone().unwrap()
        } else {
            content.replace('\n', " ").into()
        };
        let run = gpui::TextRun {
            len: display_text.len(),
            font: style.font(),
            color: if is_placeholder {
                // Muted hint — works on both dark and light themes via parent text_color blending;
                // use a semi-transparent version of the current text color.
                style.color.opacity(0.45)
            } else {
                style.color
            },
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            let marked = marked_range.start.min(display_text.len())
                ..marked_range.end.min(display_text.len());
            vec![
                gpui::TextRun {
                    len: marked.start,
                    ..run.clone()
                },
                gpui::TextRun {
                    len: marked.end - marked.start,
                    underline: Some(gpui::UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                gpui::TextRun {
                    len: display_text.len() - marked.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor.min(line.text.len()));
        let (selection, cursor_quad) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        gpui::size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    gpui::blue(),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left()
                                + line.x_for_index(selected_range.start.min(line.text.len())),
                            bounds.top(),
                        ),
                        point(
                            bounds.left()
                                + line.x_for_index(selected_range.end.min(line.text.len())),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(0x3311ff30),
                )),
                None,
            )
        };
        PrepaintState {
            lines: Vec::new(),
            cursor: cursor_quad,
            selections: Vec::new(),
            line: Some(line),
            selection,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        // Multiline path
        if !prepaint.lines.is_empty() {
            for sel in prepaint.selections.drain(..) {
                window.paint_quad(sel);
            }
            let line_height = window.line_height();
            let stored = prepaint.lines.clone();
            self.input.update(cx, |input, _cx| {
                input.last_lines = Some(stored);
                input.last_bounds = Some(bounds);
                input.last_layout = None;
            });
            for (idx, line) in prepaint.lines.drain(..).enumerate() {
                let y = bounds.top() + line_height * idx as f32;
                let origin = point(bounds.left(), y);
                line.paint(origin, line_height, window, cx).unwrap();
            }
            if focus_handle.is_focused(window) {
                if let Some(cursor) = prepaint.cursor.take() {
                    window.paint_quad(cursor);
                }
            }
            return;
        }
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        line.paint(bounds.origin, window.line_height(), window, cx)
            .unwrap();

        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
            input.last_lines = None;
        });
    }
}

impl Render for BlockInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .key_context("BlockInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::split))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for BlockInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
