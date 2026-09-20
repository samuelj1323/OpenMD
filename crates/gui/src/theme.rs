use gpui::{rgb, rgba, Rgba};

/// Flat theme for the editor. Toggle with the button in the sidebar.
#[derive(Clone, Copy)]
pub struct Theme {
    pub bg: Rgba,
    pub sidebar: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub accent: Rgba,
    pub border: Rgba,
    pub input_bg: Rgba,
    /// Row highlight for the selected page.
    pub select: Rgba,
    /// Row highlight on hover.
    pub hover: Rgba,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            bg: rgb(0x1e1e1e),
            sidebar: rgb(0x252526),
            text: rgb(0xe4e4e4),
            muted: rgb(0x9a9a9a),
            accent: rgb(0x4d9fff),
            border: rgb(0x3a3a3a),
            input_bg: rgb(0x252526),
            select: rgba(0x4d9fff40),
            hover: rgba(0x4d9fff22),
        }
    }

    pub fn light() -> Self {
        Self {
            bg: rgb(0xffffff),
            sidebar: rgb(0xf3f3f3),
            text: rgb(0x1c1c1c),
            muted: rgb(0x777777),
            accent: rgb(0x0b6cff),
            border: rgb(0xe0e0e0),
            input_bg: rgb(0xf7f7f7),
            select: rgba(0x0b6cff26),
            hover: rgba(0x0b6cff14),
        }
    }
}
