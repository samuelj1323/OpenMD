//! OpenMD — native Notion-like Markdown editor on GPUI.
//!
//! Usage: `openmd-gui [vault]` (defaults to `~/openMD`).

mod actions;
mod text_input;
mod theme;
mod workspace;

use std::path::PathBuf;

use actions::{
    Backspace, Copy, CopyMarkdown, Cut, Delete, End, ExportPage, Home, Left, NewPage, Paste, Quit,
    RevealVault, Right, Save, SelectAll, SelectLeft, SelectRight, SlashDown, SlashEscape, SlashUp,
    SplitBlock, ToggleSidebar,
};
use gpui::{
    prelude::*, px, size, App, Application, Bounds, KeyBinding, WindowBounds, WindowOptions,
};
use openmd_storage::Vault;
use workspace::WorkspaceView;

fn default_vault() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("openMD"))
        .unwrap_or_else(|| PathBuf::from("vault-example"))
}

fn main() {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_vault);
    let vault = Vault::open(&root).unwrap_or_else(|e| {
        eprintln!("OpenMD: cannot open vault {}: {e}", root.display());
        std::process::exit(1);
    });

    Application::new().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, None),
            KeyBinding::new("delete", Delete, None),
            KeyBinding::new("left", Left, None),
            KeyBinding::new("right", Right, None),
            KeyBinding::new("shift-left", SelectLeft, None),
            KeyBinding::new("shift-right", SelectRight, None),
            KeyBinding::new("cmd-a", SelectAll, None),
            KeyBinding::new("cmd-v", Paste, None),
            KeyBinding::new("cmd-c", Copy, None),
            KeyBinding::new("cmd-x", Cut, None),
            KeyBinding::new("home", Home, None),
            KeyBinding::new("end", End, None),
            KeyBinding::new("enter", SplitBlock, None),
            KeyBinding::new("up", SlashUp, None),
            KeyBinding::new("down", SlashDown, None),
            KeyBinding::new("escape", SlashEscape, None),
            KeyBinding::new("cmd-s", Save, None),
            KeyBinding::new("cmd-n", NewPage, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-b", ToggleSidebar, None),
            KeyBinding::new("cmd-\\", ToggleSidebar, None),
            KeyBinding::new("cmd-shift-c", CopyMarkdown, None),
            KeyBinding::new("cmd-shift-e", ExportPage, None),
            KeyBinding::new("cmd-shift-r", RevealVault, None),
        ]);

        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| WorkspaceView::new(vault, cx)),
            )
            .unwrap();
        window
            .update(cx, |view, window, cx| view.initial_focus(window, cx))
            .unwrap();
        cx.activate(true);
        cx.on_action(|_: &Quit, cx| cx.quit());
    });
}
