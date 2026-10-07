//! The TUI's colour palettes, selected with `:theme`.
//!
//! Rules:
//! - Colours are explicit RGB, never crossterm's named ANSI colours (those
//!   index a per-terminal table and render inconsistently). De-emphasis is
//!   the `dim` colour, not the terminal's `Dim` attribute, which some
//!   terminals ignore. `reverse` is the only plain attribute used.
//! - Every character `draw.rs` prints carries an explicit `bg` and `fg` (or
//!   an accent on `bg`), so a palette renders the same regardless of the
//!   terminal's default colours.
//! - `dark`, `light` and `hi-contrast` share one hue per role (assertion
//!   red, derived amber, core green). `colorblind` uses the Okabe-Ito
//!   palette (<https://jfly.uni-koeln.de/color/>) instead: vermillion,
//!   yellow, bluish green, distinguishable under all common colour-vision
//!   deficiencies.

use crossterm::style::Color;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb { r, g, b }
}

/// One palette's worth of named color slots.
#[derive(Clone, Copy)]
pub struct Theme {
    /// Background of every cell in the frame.
    pub bg: Color,
    /// Foreground for plain (unaccented) text.
    pub fg: Color,
    /// De-emphasized text: synthesized preamble, placeholders, the
    /// unselected suggestion strip, scrollbar tracks, breakpoint hover
    /// previews.
    pub dim: Color,
    /// Variable-name accent in formula, database and proof-rule text.
    pub variable: Color,
    /// `@label` accent in formula, database and proof-rule text.
    pub label: Color,
    /// `:edit` browse mode's `edit>` prompt label; pairs with `cursor_bg`.
    pub cursor_accent: Color,
    /// Background behind `:edit` browse mode's current cursor row in the
    /// Proof pane.
    pub cursor_bg: Color,
    /// Background behind an `a`-rule (unchecked assertion) row in the
    /// Proof pane.
    pub assertion_bg: Color,
    /// Background behind a core constraint's row in the Database pane.
    pub core_bg: Color,
    /// Background behind a derived constraint's row in the Database pane.
    pub derived_bg: Color,
    /// Background behind the buffer line `:verify` most recently rejected
    /// (`Session::known_bad`) in the Proof pane. A stronger shade of the
    /// `assertion_bg` hue; the two never coincide (an `a`-rule is never
    /// rejected).
    pub error_bg: Color,
    /// Foreground of the active-breakpoint marker in the Proof pane's
    /// numbering column. Must stay legible over any row background (see
    /// `draw::render_prefix`).
    pub breakpoint: Color,
    /// Foreground for failure text in the scrollback: `Error:` lines,
    /// rejection reasons, `:explain`'s "still fails" verdict.
    pub error_fg: Color,
    /// Foreground for success text in the scrollback (`:explain`'s "DOES
    /// check" verdict). Must stay distinguishable from `error_fg`.
    pub ok_fg: Color,
}

/// The default palette, for dark terminals. Base and accent colours are
/// from "One Dark"; highlight backgrounds are muted, darkened variants of
/// the same hues.
pub const DARK: Theme = Theme {
    bg: rgb(0x28, 0x2C, 0x34),
    fg: rgb(0xAB, 0xB2, 0xBF),
    dim: rgb(0x5C, 0x63, 0x70),
    variable: rgb(0x56, 0xB6, 0xC2),
    label: rgb(0xE5, 0xC0, 0x7B),
    cursor_accent: rgb(0xC6, 0x78, 0xDD),
    cursor_bg: rgb(0x3B, 0x2C, 0x42),
    assertion_bg: rgb(0x4A, 0x25, 0x29),
    core_bg: rgb(0x25, 0x3D, 0x28),
    derived_bg: rgb(0x4A, 0x3E, 0x10),
    error_bg: rgb(0x7F, 0x1D, 0x1D),
    breakpoint: rgb(0xE0, 0x6C, 0x75),
    error_fg: rgb(0xE0, 0x6C, 0x75),
    ok_fg: rgb(0x98, 0xC3, 0x79),
};

/// Light-background palette, from "One Light": near-white `bg`, near-black
/// `fg`, and pale tints for highlight backgrounds.
pub const LIGHT: Theme = Theme {
    bg: rgb(0xFA, 0xFA, 0xFA),
    fg: rgb(0x1A, 0x1A, 0x1A),
    dim: rgb(0xA0, 0xA1, 0xA7),
    variable: rgb(0x01, 0x84, 0xBC),
    label: rgb(0xC1, 0x84, 0x01),
    cursor_accent: rgb(0xA6, 0x26, 0xA4),
    cursor_bg: rgb(0xE9, 0xD8, 0xF5),
    assertion_bg: rgb(0xF8, 0xD7, 0xDA),
    core_bg: rgb(0xD4, 0xED, 0xDA),
    derived_bg: rgb(0xF6, 0xE6, 0xBE),
    error_bg: rgb(0xFC, 0xA5, 0xA5),
    breakpoint: rgb(0xE4, 0x56, 0x49),
    error_fg: rgb(0xE4, 0x56, 0x49),
    ok_fg: rgb(0x50, 0xA1, 0x4F),
};

/// High-contrast palette: black `bg`, white `fg`, a brighter `dim`, and
/// fully saturated accents and highlight backgrounds. Red is reserved for
/// `assertion_bg`; `derived_bg` is amber.
pub const HI_CONTRAST: Theme = Theme {
    bg: rgb(0x00, 0x00, 0x00),
    fg: rgb(0xFF, 0xFF, 0xFF),
    dim: rgb(0xB0, 0xB0, 0xB0),
    variable: rgb(0x00, 0xD7, 0xD7),
    label: rgb(0xFF, 0xD7, 0x00),
    cursor_accent: rgb(0xD7, 0x00, 0xFF),
    cursor_bg: rgb(0x5F, 0x00, 0x87),
    assertion_bg: rgb(0x87, 0x00, 0x00),
    core_bg: rgb(0x00, 0x5F, 0x00),
    derived_bg: rgb(0xB8, 0x5C, 0x00),
    error_bg: rgb(0xFF, 0x00, 0x00),
    breakpoint: rgb(0xFF, 0xA5, 0x00),
    error_fg: rgb(0xFF, 0x5F, 0x5F),
    ok_fg: rgb(0x00, 0xFF, 0x5F),
};

/// Colour-vision-deficiency-safe palette: `DARK`'s base (`bg`/`fg`/`dim`)
/// with Okabe-Ito accents. `core_bg`/`assertion_bg`/`derived_bg` are
/// bluish green/vermillion/yellow, each channel scaled by ~0.35 from the
/// canonical Okabe-Ito value.
pub const COLORBLIND: Theme = Theme {
    bg: rgb(0x28, 0x2C, 0x34),
    fg: rgb(0xAB, 0xB2, 0xBF),
    dim: rgb(0x5C, 0x63, 0x70),
    variable: rgb(0x56, 0xB4, 0xE9),
    label: rgb(0xF0, 0xE4, 0x42),
    cursor_accent: rgb(0xCC, 0x79, 0xA7),
    cursor_bg: rgb(0x47, 0x2A, 0x3A),
    assertion_bg: rgb(0x4B, 0x21, 0x00),
    core_bg: rgb(0x00, 0x37, 0x28),
    derived_bg: rgb(0x54, 0x50, 0x17),
    error_bg: rgb(0xD5, 0x5E, 0x00),
    breakpoint: rgb(0xE6, 0x9F, 0x00),
    // Okabe-Ito vermillion / bluish green for bad / good.
    error_fg: rgb(0xD5, 0x5E, 0x00),
    ok_fg: rgb(0x00, 0x9E, 0x73),
};

/// The active palette, set by `:theme <name>` (TUI only).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ThemeName {
    #[default]
    Dark,
    Light,
    HiContrast,
    Colorblind,
}

impl ThemeName {
    pub const ALL: [ThemeName; 4] = [
        ThemeName::Dark,
        ThemeName::Light,
        ThemeName::HiContrast,
        ThemeName::Colorblind,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ThemeName::Dark => "dark",
            ThemeName::Light => "light",
            ThemeName::HiContrast => "hi-contrast",
            ThemeName::Colorblind => "colorblind",
        }
    }

    /// Case-sensitive lookup by [`ThemeName::name`].
    pub fn parse(s: &str) -> Option<ThemeName> {
        ThemeName::ALL.into_iter().find(|t| t.name() == s)
    }

    pub fn theme(self) -> Theme {
        match self {
            ThemeName::Dark => DARK,
            ThemeName::Light => LIGHT,
            ThemeName::HiContrast => HI_CONTRAST,
            ThemeName::Colorblind => COLORBLIND,
        }
    }
}
