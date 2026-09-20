//! openmd-core: block document model, undoable editing ops, and GFM Markdown I/O.
//!
//! The editor works on [`Block`]s (Notion-style). Markdown is the on-disk
//! interchange format: [`parse_page`] turns a `.md` file into a [`Page`],
//! [`serialize_page`] turns it back. Inline formatting (bold, links, ...)
//! is preserved as inline Markdown inside block text.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ulid::Ulid;

/// Generate a new unique id for pages and blocks.
pub fn new_id() -> String {
    Ulid::new().to_string()
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// A single checklist item inside a [`Block::Tasks`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskItem {
    pub checked: bool,
    pub text: String,
}

/// One editable unit of a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph {
        id: String,
        text: String,
    },
    Heading {
        id: String,
        level: u8,
        text: String,
    },
    Bulleted {
        id: String,
        items: Vec<String>,
    },
    Numbered {
        id: String,
        start: u64,
        items: Vec<String>,
    },
    Tasks {
        id: String,
        items: Vec<TaskItem>,
    },
    Code {
        id: String,
        language: Option<String>,
        code: String,
    },
    Quote {
        id: String,
        text: String,
    },
    Divider {
        id: String,
    },
    Table {
        id: String,
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

impl Block {
    /// This block's stable id.
    pub fn id(&self) -> &str {
        match self {
            Block::Paragraph { id, .. }
            | Block::Heading { id, .. }
            | Block::Bulleted { id, .. }
            | Block::Numbered { id, .. }
            | Block::Tasks { id, .. }
            | Block::Code { id, .. }
            | Block::Quote { id, .. }
            | Block::Divider { id }
            | Block::Table { id, .. } => id,
        }
    }

    /// Short human name of the block kind (for palettes / debugging).
    pub fn kind_name(&self) -> &'static str {
        match self {
            Block::Paragraph { .. } => "paragraph",
            Block::Heading { .. } => "heading",
            Block::Bulleted { .. } => "bulleted",
            Block::Numbered { .. } => "numbered",
            Block::Tasks { .. } => "tasks",
            Block::Code { .. } => "code",
            Block::Quote { .. } => "quote",
            Block::Divider { .. } => "divider",
            Block::Table { .. } => "table",
        }
    }

    /// Plain-text content used for search indexing and block conversion.
    pub fn text_content(&self) -> String {
        match self {
            Block::Paragraph { text, .. }
            | Block::Heading { text, .. }
            | Block::Quote { text, .. } => text.clone(),
            Block::Code { code, .. } => code.clone(),
            Block::Bulleted { items, .. } | Block::Numbered { items, .. } => items.join("\n"),
            Block::Tasks { items, .. } => items
                .iter()
                .map(|i| i.text.clone())
                .collect::<Vec<_>>()
                .join("\n"),
            Block::Divider { .. } => String::new(),
            Block::Table { header, rows, .. } => {
                let mut parts = header.clone();
                for r in rows {
                    parts.extend(r.iter().cloned());
                }
                parts.join(" ")
            }
        }
    }

    /// Replace the primary text of text-like blocks.
    /// Returns false for Divider / lists / tasks / tables.
    pub fn set_text(&mut self, text: String) -> bool {
        match self {
            Block::Paragraph { text: t, .. }
            | Block::Heading { text: t, .. }
            | Block::Quote { text: t, .. } => {
                *t = text;
                true
            }
            Block::Code { code, .. } => {
                *code = text;
                true
            }
            _ => false,
        }
    }
}

/// A page: ordered blocks plus metadata (mirrored in file frontmatter).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub id: String,
    pub title: String,
    pub blocks: Vec<Block>,
}

impl Page {
    /// New empty page with a fresh id.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            title: title.into(),
            blocks: Vec::new(),
        }
    }

    /// Title plus all block text, for the search index.
    pub fn body_text(&self) -> String {
        let mut out = self.title.clone();
        for b in &self.blocks {
            let t = b.text_content();
            if !t.is_empty() {
                out.push('\n');
                out.push_str(&t);
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Undoable document ops
// ---------------------------------------------------------------------------

const MAX_HISTORY: usize = 100;

/// Target kind for [`Document::turn_into`].
#[derive(Clone, Debug)]
pub enum TurnInto {
    Paragraph,
    Heading(u8),
    Quote,
    Code(Option<String>),
}

/// A [`Page`] with an undo/redo history over its block list.
pub struct Document {
    pub page: Page,
    undo: Vec<Vec<Block>>,
    redo: Vec<Vec<Block>>,
}

impl Document {
    /// Wrap a page in an editable document.
    pub fn new(page: Page) -> Self {
        Self {
            page,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    fn commit(&mut self) {
        self.undo.push(self.page.blocks.clone());
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Insert a block at `index` (clamped to the end).
    pub fn insert(&mut self, index: usize, block: Block) {
        self.commit();
        let i = index.min(self.page.blocks.len());
        self.page.blocks.insert(i, block);
    }

    /// Delete the block at `index`.
    pub fn delete(&mut self, index: usize) -> Option<Block> {
        if index >= self.page.blocks.len() {
            return None;
        }
        self.commit();
        Some(self.page.blocks.remove(index))
    }

    /// Move a block from `from` to `to`.
    pub fn move_block(&mut self, from: usize, to: usize) -> bool {
        let len = self.page.blocks.len();
        if from >= len || to >= len {
            return false;
        }
        if from == to {
            return true;
        }
        self.commit();
        let b = self.page.blocks.remove(from);
        self.page.blocks.insert(to, b);
        true
    }

    /// Replace a text-like block's text.
    pub fn set_text(&mut self, index: usize, text: String) -> bool {
        let editable = matches!(
            self.page.blocks.get(index),
            Some(
                Block::Paragraph { .. }
                    | Block::Heading { .. }
                    | Block::Quote { .. }
                    | Block::Code { .. }
            )
        );
        if !editable {
            return false;
        }
        self.commit();
        self.page.blocks[index].set_text(text);
        true
    }

    /// Convert a block into another kind, preserving its text.
    pub fn turn_into(&mut self, index: usize, kind: TurnInto) -> bool {
        if index >= self.page.blocks.len() {
            return false;
        }
        self.commit();
        let old = self.page.blocks.remove(index);
        let text = old.text_content();
        let old_id = old.id().to_string();
        let fresh = match kind {
            TurnInto::Paragraph => Block::Paragraph { id: old_id, text },
            TurnInto::Heading(level) => Block::Heading {
                id: old_id,
                level: level.clamp(1, 6),
                text,
            },
            TurnInto::Quote => Block::Quote { id: old_id, text },
            TurnInto::Code(language) => Block::Code {
                id: old_id,
                language,
                code: text,
            },
        };
        self.page.blocks.insert(index, fresh);
        true
    }

    /// Flip a checklist item.
    pub fn toggle_task(&mut self, block_idx: usize, item_idx: usize) -> bool {
        let exists = matches!(
            self.page.blocks.get(block_idx),
            Some(Block::Tasks { items, .. }) if items.get(item_idx).is_some()
        );
        if !exists {
            return false;
        }
        self.commit();
        if let Some(Block::Tasks { items, .. }) = self.page.blocks.get_mut(block_idx) {
            if let Some(item) = items.get_mut(item_idx) {
                item.checked = !item.checked;
            }
        }
        true
    }

    /// Undo the last op. Returns false when history is empty.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo
            .push(std::mem::replace(&mut self.page.blocks, prev));
        true
    }

    /// Redo the last undone op. Returns false when empty.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo
            .push(std::mem::replace(&mut self.page.blocks, next));
        true
    }

    /// Number of undoable steps (for UI state).
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }
}

// ---------------------------------------------------------------------------
// Markdown I/O
// ---------------------------------------------------------------------------

fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
}

fn heading_number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Quote a frontmatter scalar when needed (`title: ...` line).
fn yaml_scalar(value: &str) -> String {
    let flat = value.replace('\n', " ");
    let needs_quotes = flat.is_empty()
        || flat.contains('"')
        || flat.contains(':')
        || flat.starts_with(' ')
        || flat.ends_with(' ')
        || flat.starts_with('#');
    if needs_quotes {
        format!("\"{}\"", flat.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        flat
    }
}

/// Unquote a frontmatter scalar produced by [`yaml_scalar`].
fn unquote_scalar(value: &str) -> String {
    let v = value.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v[1..v.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        v.to_string()
    }
}

/// Split `---` frontmatter off the top of `input`.
/// Returns `(id, title, body)`.
pub fn split_frontmatter(input: &str) -> (Option<String>, Option<String>, &str) {
    let rest = match input.strip_prefix("---\n") {
        Some(r) => r,
        None => return (None, None, input),
    };
    let mut offset = 4; // byte length of "---\n"
    let mut id = None;
    let mut title = None;
    for line in rest.lines() {
        offset += line.len() + 1; // + "\n"
        if line.trim() == "---" {
            let body = &input[offset.min(input.len())..];
            let body = body.strip_prefix('\n').unwrap_or(body);
            return (id, title, body);
        }
        if let Some((k, v)) = line.split_once(':') {
            match k.trim() {
                "id" => id = Some(v.trim().to_string()),
                "title" => title = Some(unquote_scalar(v)),
                _ => {}
            }
        }
    }
    (None, None, input)
}

// --- parser internals ---

#[derive(Debug)]
struct ListData {
    ordered: bool,
    start: u64,
    items: Vec<RawItem>,
}

#[derive(Debug)]
struct RawItem {
    text: String,
    checked: Option<bool>,
}

#[derive(Debug)]
struct TableData {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    cur: Vec<String>,
    in_head: bool,
}

#[derive(Debug)]
enum Frame {
    Para(String),
    Heading { level: u8, text: String },
    Quote(Vec<String>),
    Code { lang: Option<String>, buf: String },
    Html(String),
    List(ListData),
    Item { buf: String, checked: Option<bool> },
    Table(TableData),
    Cell(String),
    Other(String),
}

struct Builder {
    frames: Vec<Frame>,
    blocks: Vec<Block>,
    link_stack: Vec<String>,
    stray: String,
}

impl Builder {
    fn new() -> Self {
        Self {
            frames: Vec::new(),
            blocks: Vec::new(),
            link_stack: Vec::new(),
            stray: String::new(),
        }
    }

    /// Nearest buffer that accepts raw text (including code/HTML).
    fn buf_mut(&mut self) -> Option<&mut String> {
        for f in self.frames.iter_mut().rev() {
            match f {
                Frame::Para(s) | Frame::Cell(s) | Frame::Html(s) | Frame::Other(s) => {
                    return Some(s)
                }
                Frame::Heading { text, .. } => return Some(text),
                Frame::Item { buf, .. } => return Some(buf),
                Frame::Code { buf, .. } => return Some(buf),
                _ => continue,
            }
        }
        None
    }

    /// True when the innermost text target is raw (code/HTML): inline
    /// markers like `**` must be dropped, not emitted.
    fn in_raw(&self) -> bool {
        for f in self.frames.iter().rev() {
            match f {
                Frame::Code { .. } | Frame::Html(_) => return true,
                Frame::Para(_)
                | Frame::Heading { .. }
                | Frame::Item { .. }
                | Frame::Cell(_)
                | Frame::Other(_) => return false,
                _ => continue,
            }
        }
        false
    }

    fn in_cell(&self) -> bool {
        for f in self.frames.iter().rev() {
            match f {
                Frame::Cell(_) => return true,
                Frame::Para(_)
                | Frame::Heading { .. }
                | Frame::Item { .. }
                | Frame::Code { .. }
                | Frame::Html(_)
                | Frame::Other(_) => return false,
                _ => continue,
            }
        }
        false
    }

    fn push_text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if let Some(b) = self.buf_mut() {
            b.push_str(s);
        } else {
            self.stray.push_str(s);
        }
    }

    fn push_marker(&mut self, s: &str) {
        if self.in_raw() {
            return;
        }
        self.push_text(s);
    }

    fn top_is(&self, pred: impl FnOnce(&Frame) -> bool) -> bool {
        self.frames.last().is_some_and(pred)
    }

    fn top_table_mut(&mut self) -> Option<&mut TableData> {
        for f in self.frames.iter_mut().rev() {
            if let Frame::Table(t) = f {
                return Some(t);
            }
        }
        None
    }

    fn mark_task(&mut self, checked: bool) {
        for f in self.frames.iter_mut().rev() {
            if let Frame::Item { checked: c, .. } = f {
                *c = Some(checked);
                return;
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.frames.push(Frame::Para(String::new())),
            Tag::Heading { level, .. } => self.frames.push(Frame::Heading {
                level: heading_number(level),
                text: String::new(),
            }),
            Tag::BlockQuote(_) => self.frames.push(Frame::Quote(Vec::new())),
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        let s = info.into_string();
                        let first = s.split_whitespace().next().unwrap_or("").to_string();
                        if first.is_empty() {
                            None
                        } else {
                            Some(first)
                        }
                    }
                    CodeBlockKind::Indented => None,
                };
                self.frames.push(Frame::Code {
                    lang,
                    buf: String::new(),
                });
            }
            Tag::HtmlBlock => self.frames.push(Frame::Html(String::new())),
            Tag::List(first) => self.frames.push(Frame::List(ListData {
                ordered: first.is_some(),
                start: first.unwrap_or(1),
                items: Vec::new(),
            })),
            Tag::Item => self.frames.push(Frame::Item {
                buf: String::new(),
                checked: None,
            }),
            Tag::FootnoteDefinition(_) => self.frames.push(Frame::Other(String::new())),
            Tag::Table(_) => self.frames.push(Frame::Table(TableData {
                header: Vec::new(),
                rows: Vec::new(),
                cur: Vec::new(),
                in_head: false,
            })),
            Tag::TableHead => {
                if let Some(t) = self.top_table_mut() {
                    t.in_head = true;
                }
            }
            Tag::TableRow => {
                if let Some(t) = self.top_table_mut() {
                    t.cur.clear();
                }
            }
            Tag::TableCell => self.frames.push(Frame::Cell(String::new())),
            Tag::Emphasis => self.push_marker("*"),
            Tag::Strong => self.push_marker("**"),
            Tag::Strikethrough => self.push_marker("~~"),
            Tag::Link { dest_url, .. } => {
                self.push_marker("[");
                self.link_stack.push(dest_url.into_string());
            }
            Tag::Image { dest_url, .. } => {
                self.push_marker("![");
                self.link_stack.push(dest_url.into_string());
            }
            _ => {}
        }
    }

    fn flush_list(&mut self, list: ListData) {
        if list.items.is_empty() {
            return;
        }
        if list.items.iter().any(|i| i.checked.is_some()) {
            self.blocks.push(Block::Tasks {
                id: new_id(),
                items: list
                    .items
                    .into_iter()
                    .map(|i| TaskItem {
                        checked: i.checked.unwrap_or(false),
                        text: i.text,
                    })
                    .collect(),
            });
        } else if list.ordered {
            self.blocks.push(Block::Numbered {
                id: new_id(),
                start: list.start,
                items: list.items.into_iter().map(|i| i.text).collect(),
            });
        } else {
            self.blocks.push(Block::Bulleted {
                id: new_id(),
                items: list.items.into_iter().map(|i| i.text).collect(),
            });
        }
    }

    fn end(&mut self, end: TagEnd) {
        match end {
            TagEnd::Paragraph => {
                if !self.top_is(|f| matches!(f, Frame::Para(_))) {
                    return;
                }
                let text = match self.frames.pop() {
                    Some(Frame::Para(s)) => s,
                    _ => unreachable!(),
                };
                match self.frames.last_mut() {
                    Some(Frame::Item { buf, .. }) => {
                        if !buf.is_empty() {
                            buf.push('\n');
                        }
                        buf.push_str(&text);
                    }
                    Some(Frame::Quote(lines)) => lines.push(text),
                    Some(Frame::Cell(cell)) => {
                        if !cell.is_empty() {
                            cell.push(' ');
                        }
                        cell.push_str(&text);
                    }
                    Some(Frame::Other(o)) => {
                        if !o.is_empty() {
                            o.push('\n');
                        }
                        o.push_str(&text);
                    }
                    _ => {
                        if !text.trim().is_empty() {
                            self.blocks.push(Block::Paragraph { id: new_id(), text });
                        }
                    }
                }
            }
            TagEnd::Heading(_) => {
                if !self.top_is(|f| matches!(f, Frame::Heading { .. })) {
                    return;
                }
                if let Some(Frame::Heading { level, text }) = self.frames.pop() {
                    self.blocks.push(Block::Heading {
                        id: new_id(),
                        level,
                        text,
                    });
                }
            }
            TagEnd::BlockQuote(_) => {
                if !self.top_is(|f| matches!(f, Frame::Quote(_))) {
                    return;
                }
                if let Some(Frame::Quote(lines)) = self.frames.pop() {
                    let text = lines.join("\n");
                    if !text.trim().is_empty() {
                        self.blocks.push(Block::Quote { id: new_id(), text });
                    }
                }
            }
            TagEnd::CodeBlock => {
                if !self.top_is(|f| matches!(f, Frame::Code { .. })) {
                    return;
                }
                if let Some(Frame::Code { lang, buf }) = self.frames.pop() {
                    let code = buf.strip_suffix('\n').unwrap_or(&buf).to_string();
                    self.blocks.push(Block::Code {
                        id: new_id(),
                        language: lang,
                        code,
                    });
                }
            }
            TagEnd::HtmlBlock => {
                if !self.top_is(|f| matches!(f, Frame::Html(_))) {
                    return;
                }
                if let Some(Frame::Html(html)) = self.frames.pop() {
                    if !html.trim().is_empty() {
                        self.blocks.push(Block::Paragraph {
                            id: new_id(),
                            text: html,
                        });
                    }
                }
            }
            TagEnd::List(_) => {
                if !self.top_is(|f| matches!(f, Frame::List(_))) {
                    return;
                }
                if let Some(Frame::List(list)) = self.frames.pop() {
                    self.flush_list(list);
                }
            }
            TagEnd::Item => {
                if !self.top_is(|f| matches!(f, Frame::Item { .. })) {
                    return;
                }
                let item = match self.frames.pop() {
                    Some(Frame::Item { buf, checked }) => RawItem { text: buf, checked },
                    _ => unreachable!(),
                };
                let mut item = Some(item);
                for f in self.frames.iter_mut().rev() {
                    if let Frame::List(list) = f {
                        list.items.push(item.take().expect("item present"));
                        break;
                    }
                }
                if let Some(item) = item {
                    if !item.text.trim().is_empty() {
                        self.blocks.push(Block::Paragraph {
                            id: new_id(),
                            text: item.text,
                        });
                    }
                }
            }
            TagEnd::Table => {
                if !self.top_is(|f| matches!(f, Frame::Table(_))) {
                    return;
                }
                if let Some(Frame::Table(t)) = self.frames.pop() {
                    self.blocks.push(Block::Table {
                        id: new_id(),
                        header: t.header,
                        rows: t.rows,
                    });
                }
            }
            TagEnd::TableHead => {
                // 0.13 emits header cells directly under TableHead
                // (no TableRow wrapper); claim them as the header.
                if let Some(t) = self.top_table_mut() {
                    t.header = std::mem::take(&mut t.cur);
                    t.in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.top_table_mut() {
                    let row = std::mem::take(&mut t.cur);
                    if t.in_head {
                        t.header = row;
                    } else {
                        t.rows.push(row);
                    }
                }
            }
            TagEnd::TableCell => {
                if !self.top_is(|f| matches!(f, Frame::Cell(_))) {
                    return;
                }
                if let Some(Frame::Cell(text)) = self.frames.pop() {
                    if let Some(t) = self.top_table_mut() {
                        t.cur.push(text);
                    }
                }
            }
            TagEnd::FootnoteDefinition => {
                if !self.top_is(|f| matches!(f, Frame::Other(_))) {
                    return;
                }
                if let Some(Frame::Other(text)) = self.frames.pop() {
                    if !text.trim().is_empty() {
                        self.blocks.push(Block::Paragraph { id: new_id(), text });
                    }
                }
            }
            TagEnd::Emphasis => self.push_marker("*"),
            TagEnd::Strong => self.push_marker("**"),
            TagEnd::Strikethrough => self.push_marker("~~"),
            TagEnd::Link | TagEnd::Image => {
                if let Some(dest) = self.link_stack.pop() {
                    if !self.in_raw() {
                        self.push_text(&format!("]({dest})"));
                    }
                } else if !self.in_raw() {
                    self.push_text("]");
                }
            }
            _ => {}
        }
    }

    fn run(&mut self, body: &str) {
        for ev in Parser::new_ext(body, markdown_options()) {
            match ev {
                Event::Start(tag) => self.start(tag),
                Event::End(end) => self.end(end),
                Event::Text(t) => self.push_text(&t),
                Event::Code(code) => {
                    if self.in_raw() {
                        self.push_text(&code);
                    } else if let Some(b) = self.buf_mut() {
                        b.push('`');
                        b.push_str(&code);
                        b.push('`');
                    } else {
                        self.stray.push_str(&code);
                    }
                }
                Event::Html(html) | Event::InlineHtml(html) => self.push_text(&html),
                Event::FootnoteReference(name) => {
                    if self.in_raw() {
                        self.push_text(&name);
                    } else {
                        let s = format!("[^{name}]");
                        self.push_text(&s);
                    }
                }
                Event::SoftBreak => {
                    if self.in_cell() {
                        self.push_text(" ");
                    } else {
                        self.push_text("\n");
                    }
                }
                Event::HardBreak => self.push_text("  \n"),
                Event::Rule => self.blocks.push(Block::Divider { id: new_id() }),
                Event::TaskListMarker(checked) => self.mark_task(checked),
                Event::InlineMath(m) => {
                    if self.in_raw() {
                        self.push_text(&m);
                    } else {
                        let s = format!("${m}$");
                        self.push_text(&s);
                    }
                }
                Event::DisplayMath(m) => {
                    if self.in_raw() {
                        self.push_text(&m);
                    } else {
                        let s = format!("$${m}$$");
                        self.push_text(&s);
                    }
                }
            }
        }
        if !self.stray.trim().is_empty() {
            self.blocks.push(Block::Paragraph {
                id: new_id(),
                text: std::mem::take(&mut self.stray),
            });
        }
    }
}

/// Parse a Markdown document (with optional `---` frontmatter) into a [`Page`].
pub fn parse_page(input: &str) -> Page {
    let (id, title, body) = split_frontmatter(input);
    let mut builder = Builder::new();
    builder.run(body);
    let blocks = builder.blocks;
    let title = title.or_else(|| {
        blocks.iter().find_map(|b| match b {
            Block::Heading { text, .. } => Some(text.clone()),
            _ => None,
        })
    });
    Page {
        id: id.unwrap_or_else(new_id),
        title: title.unwrap_or_else(|| "Untitled".to_string()),
        blocks,
    }
}

// --- serializer ---

fn escape_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn render_block(b: &Block, out: &mut String) {
    match b {
        Block::Paragraph { text, .. } => out.push_str(text),
        Block::Heading { level, text, .. } => {
            out.push_str(&"#".repeat((*level).clamp(1, 6) as usize));
            out.push(' ');
            out.push_str(text);
        }
        Block::Bulleted { items, .. } => {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                let mut first = true;
                for line in item.split('\n') {
                    if !first {
                        out.push_str("\n  ");
                    }
                    first = false;
                    out.push_str("- ");
                    out.push_str(line);
                }
            }
        }
        Block::Numbered { start, items, .. } => {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                let mut first = true;
                for line in item.split('\n') {
                    if !first {
                        out.push_str("\n   ");
                    }
                    first = false;
                    out.push_str(&format!("{}. ", start + i as u64));
                    out.push_str(line);
                }
            }
        }
        Block::Tasks { items, .. } => {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(if item.checked { "- [x] " } else { "- [ ] " });
                out.push_str(&item.text.replace('\n', " "));
            }
        }
        Block::Code { language, code, .. } => {
            out.push_str("```");
            if let Some(lang) = language {
                out.push_str(lang);
            }
            out.push('\n');
            out.push_str(code);
            if !code.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```");
        }
        Block::Quote { text, .. } => {
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                if line.trim().is_empty() {
                    out.push('>');
                } else {
                    out.push_str("> ");
                    out.push_str(line);
                }
            }
        }
        Block::Divider { .. } => out.push_str("***"),
        Block::Table { header, rows, .. } => {
            let width = header
                .len()
                .max(rows.iter().map(Vec::len).max().unwrap_or(0));
            fn cell(r: &[String], i: usize) -> &str {
                r.get(i).map_or("", String::as_str)
            }
            let row = |r: &[String]| {
                (0..width)
                    .map(|i| escape_cell(cell(r, i)))
                    .collect::<Vec<_>>()
                    .join(" | ")
            };
            out.push_str(&format!("| {} |", row(header)));
            out.push_str(&format!(
                "\n|{}|",
                (0..width).map(|_| " --- ").collect::<Vec<_>>().join("|")
            ));
            for r in rows {
                out.push_str(&format!("\n| {} |", row(r)));
            }
        }
    }
}

/// Serialize a [`Page`] to Markdown (frontmatter + GFM body).
pub fn serialize_page(page: &Page) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("id: {}\n", page.id));
    out.push_str(&format!("title: {}\n", yaml_scalar(&page.title)));
    out.push_str("---\n");
    if page.blocks.is_empty() {
        return out;
    }
    out.push('\n');
    let mut first = true;
    for b in &page.blocks {
        if !first {
            out.push_str("\n\n");
        }
        first = false;
        render_block(b, &mut out);
    }
    out.push('\n');
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const KITCHEN_SINK: &str = "---\n\
        id: 01J0000000000000000000000\n\
        title: Hello\n\
        ---\n\
        \n\
        # Title\n\
        \n\
        Para with **bold**, *italic*, ~~strike~~, `code`, [link](https://example.com).\n\
        \n\
        - a\n\
        - b\n\
        \n\
        1. one\n\
        2. two\n\
        \n\
        - [ ] todo\n\
        - [x] done\n\
        \n\
        > quote line 1\n\
        > quote line 2\n\
        \n\
        ```rust\n\
        fn main() {}\n\
        ```\n\
        \n\
        ***\n\
        \n\
        | h1 | h2 |\n\
        | --- | --- |\n\
        | a | b |\n";

    fn round_trip(input: &str) -> Page {
        let p1 = parse_page(input);
        let s1 = serialize_page(&p1);
        let p2 = parse_page(&s1);
        let s2 = serialize_page(&p2);
        assert_eq!(s1, s2, "markdown not stable across round-trip");
        p2
    }

    #[test]
    fn kitchen_sink_round_trip() {
        let page = round_trip(KITCHEN_SINK);
        assert_eq!(page.id, "01J0000000000000000000000");
        assert_eq!(page.title, "Hello");
        assert_eq!(page.blocks.len(), 9);
        assert!(
            matches!(&page.blocks[0], Block::Heading { level: 1, text, .. } if text == "Title")
        );
        assert!(matches!(&page.blocks[1], Block::Paragraph { text, .. }
            if text.contains("**bold**") && text.contains("[link](https://example.com)")));
        assert!(matches!(&page.blocks[2], Block::Bulleted { items, .. } if items == &["a", "b"]));
        assert!(
            matches!(&page.blocks[3], Block::Numbered { items, .. } if items == &["one", "two"])
        );
        assert!(matches!(&page.blocks[4], Block::Tasks { items, .. }
            if items.len() == 2 && !items[0].checked && items[1].checked));
        assert!(
            matches!(&page.blocks[5], Block::Quote { text, .. } if text.contains("quote line 1"))
        );
        assert!(matches!(&page.blocks[6], Block::Code { language, code, .. }
            if language.as_deref() == Some("rust") && code.contains("fn main()")));
        assert!(matches!(&page.blocks[7], Block::Divider { .. }));
        assert!(matches!(&page.blocks[8], Block::Table { header, rows, .. }
            if header == &["h1", "h2"] && rows == &[vec!["a".to_string(), "b".to_string()]]));
    }

    #[test]
    fn title_falls_back_to_first_heading() {
        let page = parse_page("# My Doc\n\ntext\n");
        assert_eq!(page.title, "My Doc");
    }

    #[test]
    fn empty_document() {
        let page = parse_page("");
        assert_eq!(page.title, "Untitled");
        assert!(page.blocks.is_empty());
        let s = serialize_page(&page);
        assert!(s.starts_with("---\n"));
        round_trip(&s);
    }

    #[test]
    fn frontmatter_title_with_colon_round_trips() {
        let mut page = Page::new("Notes: weekly review");
        page.blocks.push(Block::Paragraph {
            id: new_id(),
            text: "hi".to_string(),
        });
        let s = serialize_page(&page);
        let back = parse_page(&s);
        assert_eq!(back.title, "Notes: weekly review");
        assert_eq!(back.id, page.id);
    }

    #[test]
    fn document_ops_and_undo() {
        let mut doc = Document::new(Page::new("t"));
        doc.insert(
            0,
            Block::Paragraph {
                id: new_id(),
                text: "a".into(),
            },
        );
        doc.insert(
            1,
            Block::Paragraph {
                id: new_id(),
                text: "b".into(),
            },
        );
        assert!(doc.turn_into(0, TurnInto::Heading(2)));
        assert!(matches!(
            &doc.page.blocks[0],
            Block::Heading { level: 2, .. }
        ));
        assert!(doc.undo());
        assert!(matches!(&doc.page.blocks[0], Block::Paragraph { .. }));
        assert!(doc.redo());
        assert!(matches!(
            &doc.page.blocks[0],
            Block::Heading { level: 2, .. }
        ));
        assert!(doc.delete(1).is_some());
        assert_eq!(doc.page.blocks.len(), 1);
        assert!(doc.undo());
        assert_eq!(doc.page.blocks.len(), 2);
        assert!(doc.move_block(0, 1));
        assert!(matches!(&doc.page.blocks[1], Block::Heading { .. }));
    }

    #[test]
    fn task_toggle_round_trip() {
        let mut doc = Document::new(Page::new("t"));
        doc.insert(
            0,
            Block::Tasks {
                id: new_id(),
                items: vec![TaskItem {
                    checked: false,
                    text: "do it".into(),
                }],
            },
        );
        assert!(doc.toggle_task(0, 0));
        assert!(matches!(&doc.page.blocks[0], Block::Tasks { items, .. } if items[0].checked));
        assert!(doc.undo());
        assert!(matches!(&doc.page.blocks[0], Block::Tasks { items, .. } if !items[0].checked));
        let s = serialize_page(&doc.page);
        assert!(s.contains("- [ ] do it"));
    }

    #[test]
    fn no_frontmatter_still_parses() {
        let page = parse_page("Just some text.\n");
        assert_eq!(page.blocks.len(), 1);
        assert!(matches!(&page.blocks[0], Block::Paragraph { .. }));
    }
}
