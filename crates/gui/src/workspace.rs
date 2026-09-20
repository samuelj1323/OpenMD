//! Workspace view: sidebar page list + block editor column.
//!
//! The [`openmd_core::Page`] is the source of truth. Editable blocks each
//! own a [`BlockInput`](crate::text_input::BlockInput) entity; edits flow
//! back as [`BlockEvent`](crate::text_input::BlockEvent)s and autosave.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, MouseButton, MouseUpEvent,
    Subscription, Window,
};
use openmd_core::{new_id, Block, Page};
use openmd_storage::{PageMeta, Vault};

use crate::actions::{CopyMarkdown, ExportPage, NewPage, RevealVault, Save, ToggleSidebar};
use crate::text_input::{BlockEvent, BlockInput};
use crate::theme::Theme;
use gpui::ClipboardItem;
use openmd_core::serialize_page;

/// Block index used for the page-title input.
pub const TITLE_IDX: usize = usize::MAX;

#[derive(Clone, Debug, PartialEq, Eq)]
enum SlashKind {
    Paragraph,
    H1,
    H2,
    H3,
    Bulleted,
    Numbered,
    Tasks,
    Quote,
    Code,
    Divider,
}

#[derive(Clone, Debug)]
struct SlashItem {
    label: &'static str,
    desc: &'static str,
    kind: SlashKind,
    keywords: &'static str,
}

#[derive(Clone, Debug)]
struct SlashState {
    block_idx: usize,
    query: String,
}

pub struct WorkspaceView {
    focus_handle: FocusHandle,
    vault: Vault,
    metas: Vec<PageMeta>,
    open_id: Option<String>,
    page: Option<Page>,
    page_path: Option<PathBuf>,
    title_editor: Option<Entity<BlockInput>>,
    // Per-block editors: one vec per block, each inner vec is 0..N item editors.
    // Paragraph / Heading / Quote / Code => 1 entry, Divider/Table => 0, Bulleted/Numbered/Tasks => N entries.
    editors: Vec<Vec<Entity<BlockInput>>>,
    subscriptions: Vec<Subscription>,
    dark: bool,
    // (block_idx, item_idx, cursor) — TITLE_IDX means title editor.
    pending_focus: Option<(usize, Option<usize>, usize)>,
    slash: Option<SlashState>,
    slash_selected: usize,
    code_lang_menu: Option<usize>,
    /// None = follow responsive breakpoint, Some(true)=forced visible, Some(false)=forced hidden.
    sidebar_user_override: Option<bool>,
    /// Transient notice for export/copy actions (e.g. "Copied Markdown").
    export_notice: Option<String>,
}

impl WorkspaceView {
    pub fn new(vault: Vault, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            vault,
            metas: Vec::new(),
            open_id: None,
            page: None,
            page_path: None,
            title_editor: None,
            editors: Vec::new(),
            subscriptions: Vec::new(),
            dark: true,
            pending_focus: None,
            slash: None,
            slash_selected: 0,
            code_lang_menu: None,
            sidebar_user_override: None,
            export_notice: None,
        };
        this.refresh_metas();
        if let Some(first) = this.metas.first().cloned() {
            this.open_meta(&first, cx);
        } else {
            let mut page = Page::new("Welcome");
            page.blocks.push(Block::Paragraph {
                id: new_id(),
                text: "Welcome to OpenMD. Click here and start typing.".to_string(),
            });
            let path = this.vault.write(&page).expect("write welcome page");
            this.page_path = Some(path);
            this.page = Some(page);
            this.refresh_metas();
            this.rebuild_editors(cx);
        }
        this
    }

    /// Focus the first editable block (called once at startup).
    pub fn initial_focus(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .editors
            .iter()
            .position(|v| !v.is_empty())
            .unwrap_or(TITLE_IDX);
        if target == TITLE_IDX {
            self.pending_focus = Some((TITLE_IDX, None, 0));
        } else {
            self.pending_focus = Some((target, (self.editors[target].len() > 1).then_some(0), 0));
            // For list blocks the first item is at item_idx 0; for single blocks item_idx is None.
            if self.editors[target].len() == 1 {
                // Check whether this block is a list type with 1 item — then item_idx is Some(0)
                // Determine from page blocks.
                let is_list = self
                    .page
                    .as_ref()
                    .and_then(|p| p.blocks.get(target))
                    .is_some_and(|b| {
                        matches!(
                            b,
                            Block::Bulleted { .. } | Block::Numbered { .. } | Block::Tasks { .. }
                        )
                    });
                if is_list {
                    self.pending_focus = Some((target, Some(0), 0));
                } else {
                    self.pending_focus = Some((target, None, 0));
                }
            } else if self.editors[target].len() > 1 {
                self.pending_focus = Some((target, Some(0), 0));
            }
        }
        cx.notify();
    }

    fn refresh_metas(&mut self) {
        if let Ok(metas) = self.vault.list() {
            self.metas = metas;
        }
    }

    fn open_meta(&mut self, meta: &PageMeta, cx: &mut Context<Self>) {
        match self.vault.read(&meta.path) {
            Ok(page) => {
                self.open_id = Some(meta.id.clone());
                self.page_path = Some(meta.path.clone());
                self.page = Some(page);
                self.rebuild_editors(cx);
                cx.notify();
            }
            Err(e) => eprintln!("OpenMD: cannot read {}: {e}", meta.path.display()),
        }
    }

    fn open_by_id(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(meta) = self.metas.iter().find(|m| m.id == id).cloned() {
            self.open_meta(&meta, cx);
        }
    }

    fn rebuild_editors(&mut self, cx: &mut Context<Self>) {
        self.subscriptions.clear();
        self.editors.clear();
        self.title_editor = None;
        let Some(page) = &self.page else {
            return;
        };
        let title = cx.new(|cx| {
            BlockInput::new(TITLE_IDX, page.title.clone(), cx).with_placeholder("Untitled")
        });
        self.subscriptions
            .push(cx.subscribe(&title, Self::on_block_event));
        self.title_editor = Some(title);
        for (i, block) in page.blocks.iter().enumerate() {
            match block {
                Block::Paragraph { text, .. } => {
                    let editor = cx.new(|cx| {
                        BlockInput::new(i, text.clone(), cx)
                            .with_placeholder("Type '/' for commands…")
                    });
                    self.subscriptions
                        .push(cx.subscribe(&editor, Self::on_block_event));
                    self.editors.push(vec![editor]);
                }
                Block::Heading { text, .. } => {
                    let editor = cx.new(|cx| {
                        BlockInput::new(i, text.clone(), cx).with_placeholder("Heading…")
                    });
                    self.subscriptions
                        .push(cx.subscribe(&editor, Self::on_block_event));
                    self.editors.push(vec![editor]);
                }
                Block::Quote { text, .. } => {
                    let editor = cx.new(|cx| {
                        BlockInput::new(i, text.clone(), cx).with_placeholder("Empty quote")
                    });
                    self.subscriptions
                        .push(cx.subscribe(&editor, Self::on_block_event));
                    self.editors.push(vec![editor]);
                }
                Block::Code { code, language, .. } => {
                    let lang = language.clone();
                    let editor = cx.new(|cx| {
                        BlockInput::new(i, code.clone(), cx)
                            .with_placeholder("Type code…")
                            .multiline(true)
                            .with_language(lang)
                    });
                    self.subscriptions
                        .push(cx.subscribe(&editor, Self::on_block_event));
                    self.editors.push(vec![editor]);
                }
                Block::Bulleted { items, .. } => {
                    let mut vec_ed = Vec::new();
                    for (item_idx, item) in items.iter().enumerate() {
                        let editor = cx.new(|cx| {
                            BlockInput::new_item(i, item_idx, item.clone(), cx)
                                .with_placeholder("List item")
                        });
                        self.subscriptions
                            .push(cx.subscribe(&editor, Self::on_block_event));
                        vec_ed.push(editor);
                    }
                    // Ensure at least one editor for empty list (should not happen but safe)
                    if vec_ed.is_empty() {
                        let editor = cx.new(|cx| {
                            BlockInput::new_item(i, 0, String::new(), cx)
                                .with_placeholder("List item")
                        });
                        self.subscriptions
                            .push(cx.subscribe(&editor, Self::on_block_event));
                        vec_ed.push(editor);
                    }
                    self.editors.push(vec_ed);
                }
                Block::Numbered { items, .. } => {
                    let mut vec_ed = Vec::new();
                    for (item_idx, item) in items.iter().enumerate() {
                        let editor = cx.new(|cx| {
                            BlockInput::new_item(i, item_idx, item.clone(), cx)
                                .with_placeholder("List item")
                        });
                        self.subscriptions
                            .push(cx.subscribe(&editor, Self::on_block_event));
                        vec_ed.push(editor);
                    }
                    if vec_ed.is_empty() {
                        let editor = cx.new(|cx| {
                            BlockInput::new_item(i, 0, String::new(), cx)
                                .with_placeholder("List item")
                        });
                        self.subscriptions
                            .push(cx.subscribe(&editor, Self::on_block_event));
                        vec_ed.push(editor);
                    }
                    self.editors.push(vec_ed);
                }
                Block::Tasks { items, .. } => {
                    let mut vec_ed = Vec::new();
                    for (item_idx, item) in items.iter().enumerate() {
                        let editor = cx.new(|cx| {
                            BlockInput::new_item(i, item_idx, item.text.clone(), cx)
                                .with_placeholder("To-do")
                        });
                        self.subscriptions
                            .push(cx.subscribe(&editor, Self::on_block_event));
                        vec_ed.push(editor);
                    }
                    if vec_ed.is_empty() {
                        let editor = cx.new(|cx| {
                            BlockInput::new_item(i, 0, String::new(), cx).with_placeholder("To-do")
                        });
                        self.subscriptions
                            .push(cx.subscribe(&editor, Self::on_block_event));
                        vec_ed.push(editor);
                    }
                    self.editors.push(vec_ed);
                }
                Block::Divider { .. } | Block::Table { .. } => {
                    self.editors.push(Vec::new());
                }
            }
        }
    }

    fn on_block_event(
        &mut self,
        _: Entity<BlockInput>,
        event: &BlockEvent,
        cx: &mut Context<Self>,
    ) {
        match event.clone() {
            BlockEvent::Changed {
                block_idx,
                item_idx,
                text,
            } => {
                // List items are edited per-item.
                if let Some(item_idx) = item_idx {
                    let Some(page) = self.page.as_mut() else {
                        return;
                    };
                    let Some(block) = page.blocks.get_mut(block_idx) else {
                        return;
                    };
                    match block {
                        Block::Bulleted { items, .. } => {
                            if let Some(it) = items.get_mut(item_idx) {
                                *it = text;
                            } else {
                                return;
                            }
                        }
                        Block::Numbered { items, .. } => {
                            if let Some(it) = items.get_mut(item_idx) {
                                *it = text;
                            } else {
                                return;
                            }
                        }
                        Block::Tasks { items, .. } => {
                            if let Some(it) = items.get_mut(item_idx) {
                                it.text = text;
                            } else {
                                return;
                            }
                        }
                        _ => return,
                    }
                    self.save(cx);
                    cx.notify();
                    return;
                }
                let Some(page) = self.page.as_mut() else {
                    return;
                };
                if block_idx == TITLE_IDX {
                    page.title = text;
                    self.slash = None;
                    self.save(cx);
                    cx.notify();
                    return;
                }
                // 1) Markdown-prefix auto-convert (e.g. "- [ ] ", "# ", "> ").
                // Only for text-like blocks; keep the same block id.
                let block_id = page
                    .blocks
                    .get(block_idx)
                    .map(|b| b.id().to_string())
                    .unwrap_or_default();
                let is_text_like = matches!(
                    page.blocks.get(block_idx),
                    Some(Block::Paragraph { .. } | Block::Heading { .. } | Block::Quote { .. })
                );
                // Code blocks stay code; don't auto-convert inside them and don't show slash.
                let is_code = matches!(page.blocks.get(block_idx), Some(Block::Code { .. }));
                if is_text_like {
                    if let Some(new_block) = Self::prefix_to_block(&text, &block_id) {
                        page.blocks[block_idx] = new_block;
                        let (cursor, item) = match &page.blocks[block_idx] {
                            Block::Paragraph { text, .. }
                            | Block::Heading { text, .. }
                            | Block::Quote { text, .. } => (text.len(), None),
                            Block::Code { code, .. } => (code.len(), None),
                            Block::Bulleted { items, .. } | Block::Numbered { items, .. } => {
                                (items.first().map(|s| s.len()).unwrap_or(0), Some(0))
                            }
                            Block::Tasks { items, .. } => {
                                (items.first().map(|i| i.text.len()).unwrap_or(0), Some(0))
                            }
                            _ => (0, None),
                        };
                        self.rebuild_editors(cx);
                        self.pending_focus = Some((block_idx, item, cursor));
                        self.slash = None;
                        self.save(cx);
                        cx.notify();
                        return;
                    }
                }

                // 2) Slash "/" command detection — show the command menu for this block.
                // Don't show slash inside code blocks.
                if !is_code {
                    if let Some(query) = Self::slash_query(&text) {
                        let is_new = self
                            .slash
                            .as_ref()
                            .map(|s| s.block_idx != block_idx || s.query != query)
                            .unwrap_or(true);
                        self.slash = Some(SlashState { block_idx, query });
                        if is_new {
                            self.slash_selected = 0;
                        }
                    } else if self
                        .slash
                        .as_ref()
                        .is_some_and(|s| s.block_idx == block_idx)
                    {
                        self.slash = None;
                    }
                } else if self
                    .slash
                    .as_ref()
                    .is_some_and(|s| s.block_idx == block_idx)
                {
                    self.slash = None;
                }

                if let Some(block) = page.blocks.get_mut(block_idx) {
                    if !block.set_text(text) {
                        cx.notify();
                        return;
                    }
                } else {
                    return;
                }
                self.save(cx);
                cx.notify();
            }
            BlockEvent::Split {
                block_idx,
                item_idx,
                cursor,
            } => {
                // If the slash menu is open for this block, Enter confirms the selection.
                if self
                    .slash
                    .as_ref()
                    .is_some_and(|s| s.block_idx == block_idx)
                    && item_idx.is_none()
                {
                    let state = self.slash.clone().unwrap();
                    let items = Self::filtered_slash_items(&state.query);
                    if !items.is_empty() {
                        let idx = self.slash_selected.min(items.len() - 1);
                        let kind = items[idx].kind.clone();
                        self.apply_slash_kind(block_idx, kind, cx);
                    } else {
                        self.close_slash(cx);
                    }
                    return;
                }
                if block_idx == TITLE_IDX {
                    let target = self.editors.iter().position(|e| !e.is_empty()).unwrap_or(0);
                    // Focus first editable block's first item if it's a list.
                    let is_list = self
                        .page
                        .as_ref()
                        .and_then(|p| p.blocks.get(target))
                        .is_some_and(|b| {
                            matches!(
                                b,
                                Block::Bulleted { .. }
                                    | Block::Numbered { .. }
                                    | Block::Tasks { .. }
                            )
                        });
                    let item = if is_list { Some(0) } else { None };
                    self.pending_focus = Some((target, item, 0));
                    cx.notify();
                    return;
                }
                // List item: Enter continues the same list type.
                if let Some(item_idx) = item_idx {
                    let Some(page) = self.page.as_mut() else {
                        return;
                    };
                    if block_idx >= page.blocks.len() {
                        return;
                    }
                    match &mut page.blocks[block_idx] {
                        Block::Bulleted { items, .. } => {
                            if item_idx >= items.len() {
                                return;
                            }
                            let cur = items[item_idx].clone();
                            let cursor = cursor.min(cur.len()).min(cur.len());
                            let cursor = cur.floor_char_boundary(cursor);
                            let (left, right) = cur.split_at(cursor);
                            items[item_idx] = left.to_string();
                            let right = right.trim_start().to_string();
                            items.insert(item_idx + 1, right);
                            self.rebuild_editors(cx);
                            self.pending_focus = Some((block_idx, Some(item_idx + 1), 0));
                            self.slash = None;
                            self.save(cx);
                            cx.notify();
                            return;
                        }
                        Block::Numbered { items, .. } => {
                            if item_idx >= items.len() {
                                return;
                            }
                            let cur = items[item_idx].clone();
                            let cursor = cursor.min(cur.len());
                            let cursor = cur.floor_char_boundary(cursor);
                            let (left, right) = cur.split_at(cursor);
                            items[item_idx] = left.to_string();
                            let right = right.trim_start().to_string();
                            items.insert(item_idx + 1, right);
                            self.rebuild_editors(cx);
                            self.pending_focus = Some((block_idx, Some(item_idx + 1), 0));
                            self.slash = None;
                            self.save(cx);
                            cx.notify();
                            return;
                        }
                        Block::Tasks { items, .. } => {
                            if item_idx >= items.len() {
                                return;
                            }
                            let cur_text = items[item_idx].text.clone();
                            let cursor = cursor.min(cur_text.len());
                            let cursor = cur_text.floor_char_boundary(cursor);
                            let (left, right) = cur_text.split_at(cursor);
                            items[item_idx].text = left.to_string();
                            let right = right.trim_start().to_string();
                            items.insert(
                                item_idx + 1,
                                openmd_core::TaskItem {
                                    checked: false,
                                    text: right,
                                },
                            );
                            self.rebuild_editors(cx);
                            self.pending_focus = Some((block_idx, Some(item_idx + 1), 0));
                            self.slash = None;
                            self.save(cx);
                            cx.notify();
                            return;
                        }
                        _ => {}
                    }
                    // Fall through to block split if not a list.
                }
                let remainder = {
                    let Some(page) = self.page.as_mut() else {
                        return;
                    };
                    let Some(block) = page.blocks.get_mut(block_idx) else {
                        return;
                    };
                    let text = block.text_content();
                    let cursor = cursor.min(text.len());
                    let cursor = text.floor_char_boundary(cursor);
                    let (left, right) = text.split_at(cursor);
                    if !block.set_text(left.to_string()) {
                        return;
                    }
                    right.trim_start().to_string()
                };
                if let Some(page) = self.page.as_mut() {
                    // Continue same type for list blocks when splitting the block itself
                    // (fallback). For tasks/bulleted/numbered created via prefix, Enter on the
                    // single line should create a new block of same kind.
                    let new_block = match &page.blocks[block_idx] {
                        Block::Bulleted { .. } => Block::Bulleted {
                            id: new_id(),
                            items: vec![remainder.clone()],
                        },
                        Block::Numbered { .. } => Block::Numbered {
                            id: new_id(),
                            start: 1,
                            items: vec![remainder.clone()],
                        },
                        Block::Tasks { .. } => Block::Tasks {
                            id: new_id(),
                            items: vec![openmd_core::TaskItem {
                                checked: false,
                                text: remainder.clone(),
                            }],
                        },
                        _ => Block::Paragraph {
                            id: new_id(),
                            text: remainder.clone(),
                        },
                    };
                    // If we handled it as same-type continuation, remainder is already used.
                    // For paragraph-like we already split; for list we inserted new block.
                    // Decide whether to insert as same type or paragraph based on original.
                    let is_list_block = matches!(
                        page.blocks[block_idx],
                        Block::Bulleted { .. } | Block::Numbered { .. } | Block::Tasks { .. }
                    );
                    if is_list_block {
                        // For list block Enter at top level (not item), create new list block after.
                        page.blocks.insert(block_idx + 1, new_block);
                        self.rebuild_editors(cx);
                        self.pending_focus = Some((block_idx + 1, Some(0), 0));
                    } else {
                        page.blocks.insert(block_idx + 1, new_block);
                        self.rebuild_editors(cx);
                        self.pending_focus = Some((block_idx + 1, None, 0));
                    }
                }
                self.slash = None;
                self.save(cx);
            }
            BlockEvent::DeleteEmpty {
                block_idx,
                item_idx,
            } => {
                if block_idx == TITLE_IDX {
                    return;
                }
                // List item deletion.
                if let Some(item_idx) = item_idx {
                    let focus_target = {
                        let Some(page) = self.page.as_mut() else {
                            return;
                        };
                        if block_idx >= page.blocks.len() {
                            return;
                        }
                        let should_remove_block = match &mut page.blocks[block_idx] {
                            Block::Bulleted { items, .. } | Block::Numbered { items, .. } => {
                                if items.len() <= 1 {
                                    true
                                } else {
                                    items.remove(item_idx);
                                    false
                                }
                            }
                            Block::Tasks { items, .. } => {
                                if items.len() <= 1 {
                                    true
                                } else {
                                    items.remove(item_idx);
                                    false
                                }
                            }
                            _ => true,
                        };
                        if should_remove_block {
                            if page.blocks.len() <= 1 {
                                return;
                            }
                            page.blocks.remove(block_idx);
                            let prev = block_idx.saturating_sub(1);
                            let is_prev_list = page.blocks.get(prev).is_some_and(|b| {
                                matches!(
                                    b,
                                    Block::Bulleted { .. }
                                        | Block::Numbered { .. }
                                        | Block::Tasks { .. }
                                )
                            });
                            let (prev_item, cursor) = if is_prev_list {
                                let len = match &page.blocks[prev] {
                                    Block::Bulleted { items, .. }
                                    | Block::Numbered { items, .. } => {
                                        items.last().map(|s| s.len()).unwrap_or(0)
                                    }
                                    Block::Tasks { items, .. } => {
                                        items.last().map(|i| i.text.len()).unwrap_or(0)
                                    }
                                    _ => page.blocks[prev].text_content().len(),
                                };
                                let last_idx = match &page.blocks[prev] {
                                    Block::Bulleted { items, .. }
                                    | Block::Numbered { items, .. } => {
                                        Some(items.len().saturating_sub(1))
                                    }
                                    Block::Tasks { items, .. } => {
                                        Some(items.len().saturating_sub(1))
                                    }
                                    _ => None,
                                };
                                (last_idx, len)
                            } else {
                                (
                                    None,
                                    page.blocks
                                        .get(prev)
                                        .map(|b| b.text_content().len())
                                        .unwrap_or(0),
                                )
                            };
                            (prev, prev_item, cursor)
                        } else {
                            // Removed one item, focus previous item's end or next item's start.
                            let new_idx = item_idx.saturating_sub(1);
                            let cursor = match &page.blocks[block_idx] {
                                Block::Bulleted { items, .. } | Block::Numbered { items, .. } => {
                                    items.get(new_idx).map(|s| s.len()).unwrap_or(0)
                                }
                                Block::Tasks { items, .. } => {
                                    items.get(new_idx).map(|i| i.text.len()).unwrap_or(0)
                                }
                                _ => 0,
                            };
                            (block_idx, Some(new_idx), cursor)
                        }
                    };
                    self.rebuild_editors(cx);
                    self.pending_focus = Some(focus_target);
                    self.slash = None;
                    self.save(cx);
                    return;
                }
                let focus_target = {
                    let Some(page) = self.page.as_mut() else {
                        return;
                    };
                    if page.blocks.len() <= 1 || block_idx >= page.blocks.len() {
                        return;
                    }
                    page.blocks.remove(block_idx);
                    let prev = block_idx.saturating_sub(1);
                    let is_prev_list = page.blocks.get(prev).is_some_and(|b| {
                        matches!(
                            b,
                            Block::Bulleted { .. } | Block::Numbered { .. } | Block::Tasks { .. }
                        )
                    });
                    let (prev_item, cursor) = if is_prev_list {
                        let len = match &page.blocks[prev] {
                            Block::Bulleted { items, .. } | Block::Numbered { items, .. } => {
                                items.last().map(|s| s.len()).unwrap_or(0)
                            }
                            Block::Tasks { items, .. } => {
                                items.last().map(|i| i.text.len()).unwrap_or(0)
                            }
                            _ => page.blocks[prev].text_content().len(),
                        };
                        let last_idx = match &page.blocks[prev] {
                            Block::Bulleted { items, .. } | Block::Numbered { items, .. } => {
                                Some(items.len().saturating_sub(1))
                            }
                            Block::Tasks { items, .. } => Some(items.len().saturating_sub(1)),
                            _ => None,
                        };
                        (last_idx, len)
                    } else {
                        (
                            None,
                            page.blocks
                                .get(prev)
                                .map(|b| b.text_content().len())
                                .unwrap_or(0),
                        )
                    };
                    (prev, prev_item, cursor)
                };
                self.rebuild_editors(cx);
                self.pending_focus = Some(focus_target);
                self.slash = None;
                self.save(cx);
            }
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.page.as_ref() else {
            return;
        };
        match self.vault.write(page) {
            Ok(path) => {
                if self.page_path.as_ref() != Some(&path) {
                    if let Some(old) = self.page_path.take() {
                        std::fs::remove_file(old).ok();
                    }
                    self.page_path = Some(path);
                }
                self.open_id = Some(page.id.clone());
                self.refresh_metas();
                cx.notify();
            }
            Err(e) => eprintln!("OpenMD: save failed: {e}"),
        }
    }

    fn save_now(&mut self, _: &Save, _window: &mut Window, cx: &mut Context<Self>) {
        self.save(cx);
    }

    fn new_page(&mut self, _: &NewPage, _window: &mut Window, cx: &mut Context<Self>) {
        self.create_page(cx);
    }

    fn create_page(&mut self, cx: &mut Context<Self>) {
        let mut page = Page::new("Untitled");
        page.blocks.push(Block::Paragraph {
            id: new_id(),
            text: String::new(),
        });
        match self.vault.write(&page) {
            Ok(path) => {
                self.page_path = Some(path);
                self.open_id = Some(page.id.clone());
                self.page = Some(page);
                self.refresh_metas();
                self.rebuild_editors(cx);
                self.pending_focus = Some((TITLE_IDX, None, 0));
                cx.notify();
            }
            Err(e) => eprintln!("OpenMD: cannot create page: {e}"),
        }
    }

    fn toggle_task(&mut self, block_idx: usize, item_idx: usize, cx: &mut Context<Self>) {
        let changed = match self.page.as_mut() {
            Some(page) => match page.blocks.get_mut(block_idx) {
                Some(Block::Tasks { items, .. }) => match items.get_mut(item_idx) {
                    Some(item) => {
                        item.checked = !item.checked;
                        true
                    }
                    None => false,
                },
                _ => false,
            },
            None => false,
        };
        if changed {
            self.save(cx);
        }
    }

    fn set_code_language(
        &mut self,
        block_idx: usize,
        language: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let changed = match self.page.as_mut() {
            Some(page) => match page.blocks.get_mut(block_idx) {
                Some(Block::Code { language: lang, .. }) => {
                    *lang = language;
                    true
                }
                _ => false,
            },
            None => false,
        };
        if changed {
            self.code_lang_menu = None;
            self.rebuild_editors(cx);
            self.save(cx);
            cx.notify();
        }
    }

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.dark = !self.dark;
        cx.notify();
    }

    // -- sidebar responsive ---------------------------------------------------

    const SIDEBAR_BREAKPOINT: f32 = 768.0;

    fn is_narrow(window: &Window) -> bool {
        window.viewport_size().width < px(Self::SIDEBAR_BREAKPOINT)
    }

    fn effective_sidebar_visible(&self, window: &Window) -> bool {
        if let Some(forced) = self.sidebar_user_override {
            return forced;
        }
        !Self::is_narrow(window)
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, window: &mut Window, cx: &mut Context<Self>) {
        let effective = self.effective_sidebar_visible(window);
        let new_visible = !effective;
        let auto_visible = !Self::is_narrow(window);
        if new_visible == auto_visible {
            // Back to auto behavior.
            self.sidebar_user_override = None;
        } else {
            self.sidebar_user_override = Some(new_visible);
        }
        cx.notify();
    }

    fn copy_markdown(&mut self, _: &CopyMarkdown, _: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.page.as_ref() else {
            return;
        };
        let md = serialize_page(page);
        cx.write_to_clipboard(ClipboardItem::new_string(md));
        self.export_notice = Some("Copied Markdown to clipboard".to_string());
        cx.notify();
        // Clear notice after 2s.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.export_notice = None;
                cx.notify();
            });
        })
        .detach();
    }

    fn reveal_vault(&mut self, _: &RevealVault, _: &mut Window, cx: &mut Context<Self>) {
        let path = self.vault.root().to_path_buf();
        // Copy path to clipboard for easy access from other places.
        cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
        // Try to reveal in Finder / file manager.
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(&path).spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("explorer").arg(&path).spawn();
        }
        self.export_notice = Some(format!("Vault: {} (copied, opened)", path.display()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(3))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.export_notice = None;
                cx.notify();
            });
        })
        .detach();
    }

    fn export_page(&mut self, _: &ExportPage, _: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.page.as_ref() else {
            return;
        };
        // Export is copy + also offer file location hint — for now copy Markdown
        // and copy file path if already saved.
        let md = serialize_page(page);
        cx.write_to_clipboard(ClipboardItem::new_string(md.clone()));
        // Also copy the underlying .md file path if available.
        let msg = if let Some(path) = self.page_path.as_ref() {
            // Copy file path as well — user can paste elsewhere.
            // We keep Markdown in clipboard; notice tells them file location.
            format!("Copied MD · file: {}", path.display())
        } else {
            "Copied Markdown to clipboard".to_string()
        };
        // Also try to save a copy to ~/Downloads as a quick export (best-effort).
        if let Some(home) = std::env::var_os("HOME") {
            let downloads = PathBuf::from(home).join("Downloads");
            if downloads.exists() {
                let fname = openmd_storage::filename(page);
                let dest = downloads.join(fname);
                let _ = std::fs::write(&dest, md);
            }
        }
        self.export_notice = Some(msg);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(3))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.export_notice = None;
                cx.notify();
            });
        })
        .detach();
    }

    // -- slash menu helpers ---------------------------------------------------

    fn close_slash(&mut self, cx: &mut Context<Self>) {
        if self.slash.is_some() {
            self.slash = None;
            cx.notify();
        }
    }

    fn open_slash_for(&mut self, block_idx: usize, cx: &mut Context<Self>) {
        self.slash = Some(SlashState {
            block_idx,
            query: String::new(),
        });
        self.slash_selected = 0;
        cx.notify();
    }

    fn slash_up(&mut self, _: &crate::actions::SlashUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.slash.is_none() {
            return;
        }
        let n = Self::filtered_slash_items(self.slash.as_ref().unwrap().query.as_str()).len();
        if n == 0 {
            return;
        }
        if self.slash_selected == 0 {
            self.slash_selected = n - 1;
        } else {
            self.slash_selected -= 1;
        }
        cx.notify();
    }

    fn slash_down(
        &mut self,
        _: &crate::actions::SlashDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.slash.is_none() {
            return;
        }
        let n = Self::filtered_slash_items(self.slash.as_ref().unwrap().query.as_str()).len();
        if n == 0 {
            return;
        }
        self.slash_selected = (self.slash_selected + 1) % n;
        cx.notify();
    }

    fn slash_escape(
        &mut self,
        _: &crate::actions::SlashEscape,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_slash(cx);
    }

    fn apply_slash_kind(&mut self, block_idx: usize, kind: SlashKind, cx: &mut Context<Self>) {
        let (cursor, item) = {
            let Some(page) = self.page.as_mut() else {
                return;
            };
            if block_idx >= page.blocks.len() {
                self.slash = None;
                return;
            }
            // Strip the trailing "/query" segment from the current block's text before converting,
            // so "/heading" doesn't leave a stray "/" behind.
            let raw = page.blocks[block_idx].text_content();
            let base = Self::strip_slash_suffix(&raw).to_string();
            let id = page.blocks[block_idx].id().to_string();
            let new_block = Self::slash_kind_to_block(kind.clone(), id.clone(), base.clone());
            let cursor = match &new_block {
                Block::Paragraph { text, .. }
                | Block::Heading { text, .. }
                | Block::Quote { text, .. } => text.len(),
                Block::Code { code, .. } => code.len(),
                Block::Bulleted { items, .. } | Block::Numbered { items, .. } => {
                    items.first().map(|s| s.len()).unwrap_or(0)
                }
                Block::Tasks { items, .. } => items.first().map(|i| i.text.len()).unwrap_or(0),
                _ => 0,
            };
            let item = match &new_block {
                Block::Bulleted { .. } | Block::Numbered { .. } | Block::Tasks { .. } => Some(0),
                _ => None,
            };
            page.blocks[block_idx] = new_block;
            (cursor, item)
        };
        self.slash = None;
        self.rebuild_editors(cx);
        self.pending_focus = Some((block_idx, item, cursor));
        self.save(cx);
        cx.notify();
    }

    fn strip_slash_suffix(text: &str) -> &str {
        if let Some(pos) = text.rfind('/') {
            let before = &text[..pos];
            if pos == 0 || before.ends_with(' ') {
                return before.trim_end();
            }
        }
        text
    }

    // -- markdown prefix detection --------------------------------------------

    /// Return the query after the last valid "/" trigger, if any.
    /// Valid trigger: "/" at column 0 or preceded by a space.
    fn slash_query(text: &str) -> Option<String> {
        let pos = text.rfind('/')?;
        if pos > 0 && !text[..pos].ends_with(' ') {
            return None;
        }
        let after = &text[pos + 1..];
        if after.contains(' ') {
            // Once the user inserts a space after the slash command we treat the
            // menu as dismissed (query includes spaces would not match any command).
            // Keep it open but treat query as empty to show all.
        }
        if after.contains('/') {
            return None;
        }
        Some(after.to_string())
    }

    fn prefix_to_block(text: &str, id: &str) -> Option<Block> {
        // Tasks: "- [ ] " / "- [x] " (also accept "[] " shorthand)
        if let Some(rest) = text.strip_prefix("- [ ] ") {
            return Some(Block::Tasks {
                id: id.to_string(),
                items: vec![openmd_core::TaskItem {
                    checked: false,
                    text: rest.to_string(),
                }],
            });
        }
        if let Some(rest) = text.strip_prefix("- [x] ") {
            return Some(Block::Tasks {
                id: id.to_string(),
                items: vec![openmd_core::TaskItem {
                    checked: true,
                    text: rest.to_string(),
                }],
            });
        }
        if let Some(rest) = text.strip_prefix("- [X] ") {
            return Some(Block::Tasks {
                id: id.to_string(),
                items: vec![openmd_core::TaskItem {
                    checked: true,
                    text: rest.to_string(),
                }],
            });
        }
        if let Some(rest) = text.strip_prefix("[ ] ") {
            return Some(Block::Tasks {
                id: id.to_string(),
                items: vec![openmd_core::TaskItem {
                    checked: false,
                    text: rest.to_string(),
                }],
            });
        }
        if let Some(rest) = text.strip_prefix("[x] ") {
            return Some(Block::Tasks {
                id: id.to_string(),
                items: vec![openmd_core::TaskItem {
                    checked: true,
                    text: rest.to_string(),
                }],
            });
        }
        // Bulleted: "- " or "* "
        if let Some(rest) = text.strip_prefix("- ") {
            return Some(Block::Bulleted {
                id: id.to_string(),
                items: vec![rest.to_string()],
            });
        }
        if let Some(rest) = text.strip_prefix("* ") {
            return Some(Block::Bulleted {
                id: id.to_string(),
                items: vec![rest.to_string()],
            });
        }
        // Numbered: "1. ", "2. ", ...
        if text.len() >= 3 {
            if let Some(dot) = text.find(". ") {
                let num_part = &text[..dot];
                if !num_part.is_empty()
                    && num_part.chars().all(|c| c.is_ascii_digit())
                    && num_part.len() <= 3
                {
                    let rest = &text[dot + 2..];
                    let start: u64 = num_part.parse().unwrap_or(1);
                    return Some(Block::Numbered {
                        id: id.to_string(),
                        start,
                        items: vec![rest.to_string()],
                    });
                }
            }
        }
        // Headings: "# " .. "###### "
        if text.starts_with('#') {
            let hashes = text.chars().take_while(|&c| c == '#').count();
            if (1..=6).contains(&hashes) && text[hashes..].starts_with(' ') {
                let rest = text[hashes + 1..].to_string();
                return Some(Block::Heading {
                    id: id.to_string(),
                    level: hashes as u8,
                    text: rest,
                });
            }
        }
        // Quote: "> "
        if let Some(rest) = text.strip_prefix("> ") {
            return Some(Block::Quote {
                id: id.to_string(),
                text: rest.to_string(),
            });
        }
        // Code: "```" optionally with language
        if let Some(stripped) = text.strip_prefix("```") {
            let lang = stripped.trim();
            let lang = if lang.is_empty() {
                None
            } else {
                Some(lang.to_string())
            };
            return Some(Block::Code {
                id: id.to_string(),
                language: lang,
                code: String::new(),
            });
        }
        // Divider: "---", "***", "___"
        let trimmed = text.trim();
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            return Some(Block::Divider { id: id.to_string() });
        }
        None
    }

    // -- slash menu catalogue -------------------------------------------------

    fn slash_kind_to_block(kind: SlashKind, id: String, base: String) -> Block {
        match kind {
            SlashKind::Paragraph => Block::Paragraph { id, text: base },
            SlashKind::H1 => Block::Heading {
                id,
                level: 1,
                text: base,
            },
            SlashKind::H2 => Block::Heading {
                id,
                level: 2,
                text: base,
            },
            SlashKind::H3 => Block::Heading {
                id,
                level: 3,
                text: base,
            },
            SlashKind::Bulleted => Block::Bulleted {
                id,
                items: vec![base],
            },
            SlashKind::Numbered => Block::Numbered {
                id,
                start: 1,
                items: vec![base],
            },
            SlashKind::Tasks => Block::Tasks {
                id,
                items: vec![openmd_core::TaskItem {
                    checked: false,
                    text: base,
                }],
            },
            SlashKind::Quote => Block::Quote { id, text: base },
            SlashKind::Code => Block::Code {
                id,
                language: None,
                code: base,
            },
            SlashKind::Divider => Block::Divider { id },
        }
    }

    fn all_slash_items() -> Vec<SlashItem> {
        vec![
            SlashItem {
                label: "Text",
                desc: "Plain paragraph",
                kind: SlashKind::Paragraph,
                keywords: "text paragraph",
            },
            SlashItem {
                label: "Heading 1",
                desc: "# Big section heading",
                kind: SlashKind::H1,
                keywords: "h1 heading title",
            },
            SlashItem {
                label: "Heading 2",
                desc: "## Medium heading",
                kind: SlashKind::H2,
                keywords: "h2 heading",
            },
            SlashItem {
                label: "Heading 3",
                desc: "### Small heading",
                kind: SlashKind::H3,
                keywords: "h3 heading",
            },
            SlashItem {
                label: "Bulleted list",
                desc: "• Create a bulleted list",
                kind: SlashKind::Bulleted,
                keywords: "bullet list ul -",
            },
            SlashItem {
                label: "Numbered list",
                desc: "1. Create a numbered list",
                kind: SlashKind::Numbered,
                keywords: "numbered ordered list ol 1.",
            },
            SlashItem {
                label: "To-do",
                desc: "☐ Create a task / checkbox",
                kind: SlashKind::Tasks,
                keywords: "todo task checkbox check - [ ]",
            },
            SlashItem {
                label: "Quote",
                desc: "› Create a quote block",
                kind: SlashKind::Quote,
                keywords: "quote blockquote >",
            },
            SlashItem {
                label: "Code",
                desc: "Create a code block",
                kind: SlashKind::Code,
                keywords: "code ```",
            },
            SlashItem {
                label: "Divider",
                desc: "Horizontal rule ---",
                kind: SlashKind::Divider,
                keywords: "divider hr --- ***",
            },
        ]
    }

    fn filtered_slash_items(query: &str) -> Vec<SlashItem> {
        let q = query.trim().to_lowercase();
        let all = Self::all_slash_items();
        if q.is_empty() {
            return all;
        }
        all.into_iter()
            .filter(|it| {
                it.label.to_lowercase().contains(&q)
                    || it.desc.to_lowercase().contains(&q)
                    || it.keywords.contains(&q)
            })
            .collect()
    }

    fn theme(&self) -> Theme {
        if self.dark {
            Theme::dark()
        } else {
            Theme::light()
        }
    }

    fn apply_pending_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((idx, item_idx, cursor)) = self.pending_focus.take() else {
            return;
        };
        let target = if idx == TITLE_IDX {
            self.title_editor.clone()
        } else if let Some(item_idx) = item_idx {
            self.editors.get(idx).and_then(|v| v.get(item_idx).cloned())
        } else {
            self.editors.get(idx).and_then(|v| v.first().cloned())
        };
        if let Some(entity) = target {
            entity.update(cx, |input, cx| input.set_cursor(cursor, cx));
            window.focus(&entity.focus_handle(cx));
        }
    }

    fn render_sidebar(&self, theme: Theme, cx: &mut Context<Self>) -> gpui::Div {
        let mut list = div().flex().flex_col().gap_1().flex_1();
        for meta in &self.metas {
            let selected = self.open_id.as_deref() == Some(&meta.id);
            let id = meta.id.clone();
            let mut row = div()
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .text_color(theme.text)
                .child(meta.title.clone())
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(
                        move |this: &mut Self,
                              _: &MouseUpEvent,
                              _: &mut Window,
                              cx: &mut Context<Self>| {
                            this.open_by_id(id.clone(), cx);
                        },
                    ),
                );
            if selected {
                row = row.bg(theme.select);
            } else {
                row = row.hover(|s| s.bg(theme.hover));
            }
            list = list.child(row);
        }

        div()
            .w(px(240.))
            .h_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_color(theme.text).child("OpenMD"))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_color(theme.muted)
                                    .hover(|s| s.text_color(theme.text))
                                    .child(if self.dark { "☀" } else { "☾" })
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(
                                            |this: &mut Self,
                                             _: &MouseUpEvent,
                                             _: &mut Window,
                                             cx: &mut Context<Self>| {
                                                this.toggle_theme(cx);
                                            },
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_color(theme.muted)
                                    .hover(|s| s.text_color(theme.text))
                                    .child("+ New")
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(
                                            |this: &mut Self,
                                             _: &MouseUpEvent,
                                             _: &mut Window,
                                             cx: &mut Context<Self>| {
                                                this.create_page(cx);
                                            },
                                        ),
                                    ),
                            ),
                    ),
            )
            .child(div().flex_1().id("sidebar-scroll").overflow_scroll().p_2().child(list))
    }

    fn handle_gutter(&self, index: usize, theme: Theme, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .w(px(22.))
            .h(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .text_size(px(12.))
            .text_color(theme.muted)
            .cursor_pointer()
            .hover(|s| s.bg(theme.hover).text_color(theme.text))
            .child("⠿")
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(
                    move |this: &mut Self,
                          _: &MouseUpEvent,
                          _: &mut Window,
                          cx: &mut Context<Self>| {
                        this.open_slash_for(index, cx);
                    },
                ),
            )
    }

    fn render_slash_menu(
        &self,
        block_idx: usize,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let state = self.slash.as_ref()?;
        if state.block_idx != block_idx {
            return None;
        }
        let items = Self::filtered_slash_items(&state.query);
        if items.is_empty() {
            return None;
        }
        let mut list = div()
            .flex()
            .flex_col()
            .gap_px()
            .w(px(300.))
            .max_h(px(260.))
            .id("slash-menu")
            .overflow_scroll()
            .bg(theme.sidebar)
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .p_1()
            .shadow_md();
        for (i, item) in items.iter().enumerate() {
            let selected = i == self.slash_selected;
            let kind = item.kind.clone();
            let label = item.label;
            let desc = item.desc;
            let mut row = div()
                .flex()
                .flex_col()
                .px_3()
                .py_2()
                .rounded_sm()
                .cursor_pointer()
                .child(div().text_size(px(13.)).text_color(theme.text).child(label))
                .child(div().text_size(px(11.)).text_color(theme.muted).child(desc))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(
                        move |this: &mut Self,
                              _: &MouseUpEvent,
                              _: &mut Window,
                              cx: &mut Context<Self>| {
                            this.apply_slash_kind(block_idx, kind.clone(), cx);
                        },
                    ),
                );
            if selected {
                row = row.bg(theme.select);
            } else {
                row = row.hover(|s| s.bg(theme.hover));
            }
            list = list.child(row);
        }
        // Hint row
        list = list.child(
            div()
                .px_3()
                .py_1()
                .text_size(px(10.))
                .text_color(theme.muted)
                .child("↑↓ to navigate · Enter to select · Esc to close"),
        );
        Some(div().w_full().pl(px(22. + 8.)).pt_1().child(list))
    }

    fn insert_block_after(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(page) = self.page.as_mut() else {
            return;
        };
        let at = (index + 1).min(page.blocks.len());
        page.blocks.insert(
            at,
            Block::Paragraph {
                id: new_id(),
                text: String::new(),
            },
        );
        self.rebuild_editors(cx);
        self.pending_focus = Some((at, None, 0));
        self.slash = None;
        self.save(cx);
        cx.notify();
    }

    fn render_block(
        &self,
        index: usize,
        block: &Block,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let inner: gpui::Div = match block {
            Block::Paragraph { .. } => {
                let editor = self.editors.get(index).and_then(|v| v.first().cloned());
                let mut row = div()
                    .flex_1()
                    .min_h(px(30.))
                    .text_size(px(15.))
                    .text_color(theme.text);
                if let Some(editor) = editor {
                    row = row.child(editor);
                }
                row
            }
            Block::Heading { level, .. } => {
                let editor = self.editors.get(index).and_then(|v| v.first().cloned());
                let (size, height) = match level {
                    1 => (24., 40.),
                    2 => (20., 36.),
                    _ => (17., 32.),
                };
                let mut row = div()
                    .flex_1()
                    .h(px(height))
                    .text_size(px(size))
                    .text_color(theme.text);
                if let Some(editor) = editor {
                    row = row.child(editor);
                }
                row
            }
            Block::Quote { .. } => {
                let editor = self.editors.get(index).and_then(|v| v.first().cloned());
                let mut row = div()
                    .flex_1()
                    .min_h(px(30.))
                    .text_size(px(15.))
                    .border_l_2()
                    .border_color(theme.accent)
                    .pl_3()
                    .text_color(theme.muted);
                if let Some(editor) = editor {
                    row = row.child(editor);
                }
                row
            }
            Block::Code { language, .. } => {
                let editor = self.editors.get(index).and_then(|v| v.first().cloned());
                let mut col = div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .bg(theme.input_bg)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .px_3()
                    .py_2()
                    .text_size(px(13.))
                    .text_color(theme.text);
                // Header with language picker — always visible so it's obvious it's code.
                let lang_label = language.as_deref().unwrap_or("plain text").to_string();
                let lang_menu_open = self.code_lang_menu == Some(index);
                let header = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .pb_1()
                    .mb_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .px_1()
                                    .py_0()
                                    .rounded_sm()
                                    .bg(theme.sidebar)
                                    .border_1()
                                    .border_color(theme.border)
                                    .text_size(px(10.))
                                    .text_color(theme.muted)
                                    .child("</>"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.muted)
                                    .child("Code"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .items_center()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(if lang_menu_open {
                                theme.select
                            } else {
                                theme.input_bg
                            })
                            .border_1()
                            .border_color(theme.border)
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(theme.text)
                            .child(lang_label.clone())
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .text_color(theme.muted)
                                    .child(if lang_menu_open { "▴" } else { "▾" }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                                    if this.code_lang_menu == Some(index) {
                                        this.code_lang_menu = None;
                                    } else {
                                        this.code_lang_menu = Some(index);
                                    }
                                    cx.notify();
                                }),
                            ),
                    );
                col = col.child(header);
                // Language picker dropdown
                if lang_menu_open {
                    let mut menu = div()
                        .flex()
                        .flex_col()
                        .gap_px()
                        .w(px(180.))
                        .bg(theme.sidebar)
                        .border_1()
                        .border_color(theme.border)
                        .rounded_md()
                        .p_1()
                        .shadow_md();
                    for lang_opt in [
                        None,
                        Some("rust"),
                        Some("python"),
                        Some("javascript"),
                        Some("typescript"),
                        Some("bash"),
                        Some("json"),
                        Some("yaml"),
                        Some("markdown"),
                        Some("sql"),
                        Some("go"),
                        Some("html"),
                        Some("css"),
                    ] {
                        let label = lang_opt.unwrap_or("plain text");
                        let is_selected = language.as_deref() == lang_opt;
                        let lang_clone = lang_opt.map(|s| s.to_string());
                        let mut row = div()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(theme.text)
                            .child(label)
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                                    this.set_code_language(index, lang_clone.clone(), cx);
                                }),
                            );
                        if is_selected {
                            row = row.bg(theme.select);
                        } else {
                            row = row.hover(|s| s.bg(theme.hover));
                        }
                        menu = menu.child(row);
                    }
                    col = col.child(menu);
                }
                if let Some(editor) = editor {
                    col = col.child(div().w_full().min_h(px(24.)).child(editor));
                } else {
                    col = col.child(
                        div()
                            .w_full()
                            .min_h(px(24.))
                            .text_size(px(13.))
                            .text_color(theme.muted)
                            .child("Type code…  (Enter for new line)"),
                    );
                }
                // Hint for growth
                col = col.child(
                    div()
                        .w_full()
                        .text_size(px(10.))
                        .text_color(theme.muted)
                        .pt_1()
                        .child("Enter adds a new line · block grows with content"),
                );
                col
            }
            Block::Bulleted { items, .. } => {
                let editors = self.editors.get(index).cloned().unwrap_or_default();
                let mut col = div().flex().flex_col().gap_1().flex_1();
                for (i, item) in items.iter().enumerate() {
                    let editor = editors.get(i).cloned();
                    col = col.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .text_size(px(15.))
                            .text_color(theme.text)
                            .child("•")
                            .child(if let Some(ed) = editor {
                                div().flex_1().child(ed)
                            } else {
                                div().flex_1().child(item.clone())
                            }),
                    );
                }
                col
            }
            Block::Numbered { start, items, .. } => {
                let editors = self.editors.get(index).cloned().unwrap_or_default();
                let mut col = div().flex().flex_col().gap_1().flex_1();
                for (i, item) in items.iter().enumerate() {
                    let editor = editors.get(i).cloned();
                    col = col.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .text_size(px(15.))
                            .text_color(theme.text)
                            .child(format!("{}.", start + i as u64))
                            .child(if let Some(ed) = editor {
                                div().flex_1().child(ed)
                            } else {
                                div().flex_1().child(item.clone())
                            }),
                    );
                }
                col
            }
            Block::Tasks { items, .. } => {
                let editors = self.editors.get(index).cloned().unwrap_or_default();
                let mut col = div().flex().flex_col().gap_1().flex_1();
                for (item_idx, item) in items.iter().enumerate() {
                    let editor = editors.get(item_idx).cloned();
                    let (block_idx, item_idx) = (index, item_idx);
                    let box_style = if item.checked {
                        div()
                            .size(px(16.))
                            .rounded_sm()
                            .bg(theme.accent)
                            .border_1()
                            .border_color(theme.accent)
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .line_height(px(11.))
                                    .text_color(theme.bg)
                                    .child("✓"),
                            )
                    } else {
                        div()
                            .size(px(16.))
                            .rounded_sm()
                            .border_1()
                            .border_color(theme.muted)
                            .flex_shrink_0()
                    };
                    col = col.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                box_style.cursor_pointer().on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(
                                        move |this: &mut Self,
                                              _: &MouseUpEvent,
                                              _: &mut Window,
                                              cx: &mut Context<Self>| {
                                            this.toggle_task(block_idx, item_idx, cx);
                                        },
                                    ),
                                ),
                            )
                            .child(if let Some(ed) = editor {
                                div()
                                    .flex_1()
                                    .text_size(px(15.))
                                    .text_color(if item.checked {
                                        theme.muted
                                    } else {
                                        theme.text
                                    })
                                    .child(ed)
                            } else {
                                div()
                                    .flex_1()
                                    .text_size(px(15.))
                                    .text_color(if item.checked {
                                        theme.muted
                                    } else {
                                        theme.text
                                    })
                                    .child(item.text.clone())
                            }),
                    );
                }
                col
            }
            Block::Divider { .. } => div().flex_1().h(px(1.)).bg(theme.border).my_2(),
            Block::Table { header, rows, .. } => {
                let mut table = div().flex().flex_col().flex_1().text_size(px(13.));
                let mut head = div().flex().flex_row().w_full();
                for cell in header {
                    head = head.child(
                        div()
                            .flex_1()
                            .px_2()
                            .py_1()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.sidebar)
                            .text_color(theme.text)
                            .child(cell.clone()),
                    );
                }
                table = table.child(head);
                for row in rows {
                    let mut row_div = div().flex().flex_row().w_full();
                    for cell in row {
                        row_div = row_div.child(
                            div()
                                .flex_1()
                                .px_2()
                                .py_1()
                                .border_1()
                                .border_color(theme.border)
                                .text_color(theme.text)
                                .child(cell.clone()),
                        );
                    }
                    table = table.child(row_div);
                }
                table
            }
        };

        // Outer row: [handle · content] + optional slash menu below.
        let handle = self.handle_gutter(index, theme, cx);
        let mut outer = div().flex().flex_col().w_full().gap_1();
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .rounded_sm()
            .hover(|s| s.bg(theme.hover))
            .child(handle)
            .child(inner);
        outer = outer.child(row);
        if let Some(menu) = self.render_slash_menu(index, theme, cx) {
            outer = outer.child(menu);
        }
        outer
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_pending_focus(window, cx);
        let theme = self.theme();
        let sidebar_visible = self.effective_sidebar_visible(window);
        let is_narrow = Self::is_narrow(window);

        let mut column = div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .max_w(px(760.))
            .p_6()
            .text_color(theme.text);
        if let Some(editor) = self.title_editor.clone() {
            column = column.child(
                div()
                    .w_full()
                    .h(px(48.))
                    .text_size(px(28.))
                    .text_color(theme.text)
                    .child(editor),
            );
        }
        if let Some(page) = self.page.clone() {
            for (i, block) in page.blocks.iter().enumerate() {
                column = column.child(self.render_block(i, block, theme, cx));
            }
            // Bottom affordance: always-visible add-block row + markdown hint.
            let add_idx = page.blocks.len().saturating_sub(1);
            column = column.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .mt_2()
                    .opacity(0.7)
                    .hover(|s| s.opacity(1.0))
                    .child(
                        div()
                            .w(px(22.))
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .text_color(theme.muted)
                            .child("+"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .text_color(theme.muted)
                            .cursor_pointer()
                            .child("Click to add a block — type '/' for commands, or markdown like '- [ ]', '# ', '> '")
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                                    this.insert_block_after(add_idx, cx);
                                }),
                            ),
                    ),
            );
            column = column.child(
                div()
                    .w_full()
                    .text_size(px(11.))
                    .text_color(theme.muted)
                    .pt_4()
                    .child("Tip: Hover the ⋮ handle on any block to change its type. Press '/' in an empty block for the command menu. Checkboxes support '- [ ] ' and '- [x] '."),
            );
        } else {
            column = column.child(div().text_color(theme.muted).child("Select a page"));
        }

        // -- top bar with sidebar toggle at top-left + export actions -----------
        let toggle_label = if sidebar_visible { "‹" } else { "☰" };
        // Shorten vault label for display (show last two components).
        let vault_short = {
            let p = self.vault.root();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("openMD");
            let parent = p
                .parent()
                .and_then(|par| par.file_name())
                .and_then(|s| s.to_str());
            if let Some(par) = parent {
                format!("{par}/{name}")
            } else {
                name.to_string()
            }
        };
        let top_bar = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .h(px(44.))
            .px_3()
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w(px(32.))
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .border_1()
                            .border_color(theme.border)
                            .bg(if sidebar_visible {
                                theme.select
                            } else {
                                theme.input_bg
                            })
                            .hover(|s| s.bg(theme.hover))
                            .text_color(theme.text)
                            .text_size(px(14.))
                            .child(toggle_label)
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(
                                    |this: &mut Self,
                                     _: &MouseUpEvent,
                                     window: &mut Window,
                                     cx: &mut Context<Self>| {
                                        this.toggle_sidebar(&ToggleSidebar, window, cx);
                                    },
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(theme.text)
                                    .child("OpenMD"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(theme.muted)
                                    .child(vault_short),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.input_bg)
                            .hover(|s| s.bg(theme.hover))
                            .text_size(px(11.))
                            .text_color(theme.text)
                            .child("⧉ Copy MD")
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(
                                    |this: &mut Self,
                                     _: &MouseUpEvent,
                                     window: &mut Window,
                                     cx: &mut Context<Self>| {
                                        this.copy_markdown(&CopyMarkdown, window, cx);
                                    },
                                ),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.input_bg)
                            .hover(|s| s.bg(theme.hover))
                            .text_size(px(11.))
                            .text_color(theme.text)
                            .child("↗ Export")
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(
                                    |this: &mut Self,
                                     _: &MouseUpEvent,
                                     window: &mut Window,
                                     cx: &mut Context<Self>| {
                                        this.export_page(&ExportPage, window, cx);
                                    },
                                ),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.input_bg)
                            .hover(|s| s.bg(theme.hover))
                            .text_size(px(11.))
                            .text_color(theme.text)
                            .child("⌖ Reveal")
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(
                                    |this: &mut Self,
                                     _: &MouseUpEvent,
                                     window: &mut Window,
                                     cx: &mut Context<Self>| {
                                        this.reveal_vault(&RevealVault, window, cx);
                                    },
                                ),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(theme.muted)
                            .hover(|s| s.text_color(theme.text))
                            .child(if self.dark { "☀" } else { "☾" })
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(
                                    |this: &mut Self,
                                     _: &MouseUpEvent,
                                     _: &mut Window,
                                     cx: &mut Context<Self>| {
                                        this.toggle_theme(cx);
                                    },
                                ),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(theme.accent)
                            .text_color(gpui::white())
                            .text_size(px(11.))
                            .child("+ New")
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(
                                    |this: &mut Self,
                                     _: &MouseUpEvent,
                                     _: &mut Window,
                                     cx: &mut Context<Self>| {
                                        this.create_page(cx);
                                    },
                                ),
                            ),
                    ),
            );

        let editor_area = div()
            .flex_1()
            .h_full()
            .id("editor-scroll")
            .overflow_scroll()
            .items_center()
            .child(column);

        // Body: sidebar + editor, with responsive overlay when narrow.
        let body = if sidebar_visible {
            if is_narrow {
                // Overlay sidebar on narrow screens.
                div()
                    .flex_1()
                    .relative()
                    .overflow_hidden()
                    .child(editor_area)
                    .child(
                        div()
                            .absolute()
                            .top(px(0.))
                            .left(px(0.))
                            .bottom(px(0.))
                            .w(px(280.))
                            .bg(theme.sidebar)
                            .border_r_1()
                            .border_color(theme.border)
                            .shadow_lg()
                            .child(self.render_sidebar(theme, cx)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(0.))
                            .left(px(280.))
                            .right(px(0.))
                            .bottom(px(0.))
                            .bg(gpui::rgba(0x00000066))
                            .cursor_pointer()
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(
                                    |this: &mut Self,
                                     _: &MouseUpEvent,
                                     window: &mut Window,
                                     cx: &mut Context<Self>| {
                                        this.toggle_sidebar(&ToggleSidebar, window, cx);
                                    },
                                ),
                            ),
                    )
            } else {
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_sidebar(theme, cx))
                    .child(editor_area)
            }
        } else {
            div().flex_1().overflow_hidden().child(editor_area)
        };

        let mut root = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg)
            .key_context("Workspace")
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(Self::save_now))
            .on_action(cx.listener(Self::new_page))
            .on_action(cx.listener(Self::slash_up))
            .on_action(cx.listener(Self::slash_down))
            .on_action(cx.listener(Self::slash_escape))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::copy_markdown))
            .on_action(cx.listener(Self::reveal_vault))
            .on_action(cx.listener(Self::export_page))
            .child(top_bar);

        if let Some(notice) = self.export_notice.clone() {
            root = root.child(
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .bg(theme.select)
                    .border_b_1()
                    .border_color(theme.border)
                    .text_size(px(11.))
                    .text_color(theme.text)
                    .child(notice),
            );
        }
        // Hint when sidebar auto-hidden on narrow screen.
        if !sidebar_visible && is_narrow {
            root = root.child(
                div()
                    .w_full()
                    .px_3()
                    .py_1()
                    .bg(theme.sidebar)
                    .text_size(px(10.))
                    .text_color(theme.muted)
                    .child("Sidebar hidden on small screen — click ☰ to open"),
            );
        }

        root.child(body)
    }
}

impl Focusable for WorkspaceView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
