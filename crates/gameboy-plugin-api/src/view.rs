//! [`PluginView`] — the frontend-neutral panel a plugin may publish.
//!
//! # A plugin never draws
//!
//! It publishes a view and the frontend lays it out. Three reasons, and the
//! first is a bug this project actually shipped:
//!
//! 1. **The picture rectangle is sacred.** A terminal graphics image owns whole
//!    cells, so text drawn there is not *over* the picture, it is *instead of*
//!    it — and under Kitty's Unicode placeholders it removes the placeholders
//!    that make the picture update at all. A stats overlay drawn over the game
//!    image froze the game on screen; see `docs/terminal-render.md`. A plugin
//!    handed a raw drawing surface would reintroduce that on its first use.
//! 2. **There is more than one frontend.** Desktop GL, terminal and browser
//!    canvas cannot share a drawing API, but they can all render a title and
//!    some styled lines.
//! 3. **Arbitration needs a single owner.** At most one plugin view is shown at
//!    a time; the frontend picks and the user cycles. A plugin *asks* for the
//!    screen and may not get it, which is the only arrangement in which two
//!    plugins wanting it at once is not a bug.
//!
//! This is deliberately a **small** surface: a title, some coloured lines, a
//! preferred width. It is not a widget toolkit, and it will not grow into one.
//! The plugins shipped with the emulator draw richer panels because they are
//! compiled into the frontend and use its own drawing types directly; that is
//! stated plainly in `docs/plugins/writing-a-plugin.md` rather than pretended
//! away.

/// A colour a plugin may ask for. The eight ANSI colours plus their bright
/// forms and a 24-bit escape hatch — chosen because that is the intersection of
/// what a terminal, a GL frontend and a canvas can all honour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ViewColor {
    /// Whatever the frontend's ordinary foreground is.
    #[default]
    Default,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    Gray,
    DarkGray,
    Rgb(u8, u8, u8),
}

/// How one run of text is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ViewStyle {
    pub color: ViewColor,
    pub bold: bool,
    pub dim: bool,
}

impl ViewStyle {
    pub const fn new() -> ViewStyle {
        ViewStyle {
            color: ViewColor::Default,
            bold: false,
            dim: false,
        }
    }
    pub const fn fg(mut self, color: ViewColor) -> ViewStyle {
        self.color = color;
        self
    }
    pub const fn bold(mut self) -> ViewStyle {
        self.bold = true;
        self
    }
    pub const fn dim(mut self) -> ViewStyle {
        self.dim = true;
        self
    }
}

/// One line of a [`PluginView`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewLine {
    pub text: String,
    pub style: ViewStyle,
}

impl ViewLine {
    pub fn plain(text: impl Into<String>) -> ViewLine {
        ViewLine {
            text: text.into(),
            style: ViewStyle::new(),
        }
    }
    pub fn styled(text: impl Into<String>, style: ViewStyle) -> ViewLine {
        ViewLine {
            text: text.into(),
            style,
        }
    }
}

/// What a plugin wants shown when the user asks to see it.
///
/// Returned by [`Plugin::view`](crate::Plugin::view). The frontend owns where
/// it goes and how big it may be; `width` is a *request*, and a plugin must
/// still read correctly when it is not granted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginView {
    /// Panel title. The frontend prefixes the plugin's name, so this is the
    /// *rest* of the title — often empty.
    pub title: String,
    /// The body, one entry per line, already wrapped by the plugin. Lines
    /// longer than the granted width are truncated by the frontend.
    pub lines: Vec<ViewLine>,
    /// Preferred width in character cells. 0 means "no preference".
    pub width: u16,
}

impl PluginView {
    pub fn new(title: impl Into<String>) -> PluginView {
        PluginView {
            title: title.into(),
            lines: Vec::new(),
            width: 0,
        }
    }

    pub fn line(mut self, text: impl Into<String>) -> PluginView {
        self.lines.push(ViewLine::plain(text));
        self
    }

    pub fn styled_line(mut self, text: impl Into<String>, style: ViewStyle) -> PluginView {
        self.lines.push(ViewLine::styled(text, style));
        self
    }

    pub fn width(mut self, width: u16) -> PluginView {
        self.width = width;
        self
    }
}
