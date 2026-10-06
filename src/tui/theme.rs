//! The TUI's color palettes, defined as explicit RGB values rather than
//! crossterm's named ANSI colors. Those are indices into a 16-slot table
//! each terminal (and its user) configures for itself — the same call can
//! render wildly differently machine to machine, which is exactly what
//! bit an invisible suggestion strip, a too-bright database background,
//! and a too-bright assertion background, three separate times in this
//! project. An RGB value renders as that exact color everywhere on a
//! truecolor terminal, which is effectively universal now. Even
//! de-emphasis (`dim`, below) is a themed color now rather than the
//! terminal's own `Dim`/faint attribute — that attribute isn't
//! universally honored (some terminals just render it at normal
//! intensity), where a real color always shows up. `reverse` is the one
//! thing left as a plain attribute, since it inverts whatever's already
//! there rather than substituting a specific hue, so it isn't part of
//! this problem.
//!
//! Four palettes exist so far (`:theme` picks between them): `dark`, the
//! default, tuned for a dark terminal background; `light`, its opposite
//! number (white/light-grey background, black-ish text); `hi-contrast`,
//! which trades the other two's deliberately muted, calm backgrounds for
//! fully saturated ones, true black/white for `bg`/`fg`, and a brighter
//! `dim`, favoring maximum distinction over subtlety; and `colorblind`.
//! `dark`/`light`/`hi-contrast` all keep the same *hue* per role across
//! each other — assertion stays red, derived stays amber, core stays
//! green, a traffic-light reading (red = unchecked, amber = derived but
//! not yet core, green = given) so switching between *them* changes how
//! things look, not what they mean. Red-vs-green is exactly the pairing
//! hardest to tell apart under red-green color blindness (by far the most
//! common kind), so `colorblind` breaks from those three hues on purpose:
//! vermillion for assertion, yellow for derived, blue-green for core,
//! following the Okabe-Ito palette (<https://jfly.uni-koeln.de/color/>), a
//! research-backed set specifically validated to stay pairwise
//! distinguishable under protanopia, deuteranopia, and tritanopia at once,
//! rather than guessing at "safe" colors from scratch — the same
//! three-way meaning, just carried by hues that don't collide.
//!
//! `bg`/`fg` make this genuinely app-wide rather than just accent
//! colors on top of whatever the terminal happens to default to: every
//! character `draw.rs` prints — plain content, borders, scrollback text,
//! the prompt, all of it — carries an explicit `bg`/`fg` (or an accent
//! foreground on top of that same `bg`), so `light` actually renders
//! light regardless of the terminal's own default colors, not just in
//! the bits this app chooses to accent.

use crossterm::style::Color;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb { r, g, b }
}

/// One palette's worth of named color slots.
#[derive(Clone, Copy)]
pub struct Theme {
    /// The base background every cell in the frame renders on, content
    /// panes, borders, scrollback, and prompt alike.
    pub bg: Color,
    /// The base foreground for plain (unaccented) text on `bg`.
    pub fg: Color,
    /// De-emphasized text: the synthesized preamble, placeholders, the
    /// non-highlighted suggestion strip, scrollbar tracks — anything
    /// that's context rather than content.
    pub dim: Color,
    /// Variable-name accent: formula/database tokenizing, and the
    /// lookup-based highlighting of raw proof-rule text.
    pub variable: Color,
    /// `@label` accent, same two contexts.
    pub label: Color,
    /// `:edit` browse mode's `edit>` prompt label — paired with
    /// `cursor_bg` below, tying the label to the cursor row it controls.
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
    /// Background behind the one buffer line `:verify` most recently
    /// rejected (`Session::known_bad`), in the Proof pane — deliberately
    /// a stronger, more saturated version of the same hue `assertion_bg`
    /// uses rather than a
    /// different one: the two can't coincide in practice (an `a`-rule is
    /// never itself rejected), so there's no need to keep them
    /// pairwise-distinguishable the way `assertion_bg`/`core_bg`/
    /// `derived_bg` do; this just needs to read as "more urgent" than any
    /// of them.
    pub error_bg: Color,
    /// The active-breakpoint marker glyph's own foreground, in the Proof
    /// pane's numbering column — worn regardless of whatever the row
    /// underneath it is doing (checked or not, cursor row, core/derived,
    /// even mid-error), since a breakpoint is orthogonal to all of that
    /// and needs to stay legible on top of any of it (see
    /// `draw::render_prefix`). Same hue family as `error_bg` — "stop
    /// here" and "this needs attention" are close enough kin — but a
    /// brighter, more saturated foreground-grade tone than that
    /// deliberately-muted background token, the same reason `variable`/
    /// `label` are their own accents rather than reusing a background
    /// hue. A breakpoint's *hover preview* (not yet set, mouse
    /// resting over the gutter) reuses `dim` instead of a color of its
    /// own — a preview is context, not committed state, exactly `dim`'s
    /// existing job.
    pub breakpoint: Color,
    /// Foreground for failure text in the scrollback: `Error:` lines, a
    /// rejection's reason, `:explain`'s "still fails" verdict. A
    /// foreground-grade tone, like `breakpoint`, not `error_bg`'s muted
    /// background one.
    pub error_fg: Color,
    /// Foreground for success text in the scrollback — `:explain`'s
    /// "Without hints it DOES check" verdict. Paired with `error_fg`, so
    /// the two must stay distinguishable in every palette.
    pub ok_fg: Color,
}

/// The default palette. `bg`/`fg` and the foreground accents (`dim`
/// included) are all borrowed from the well-known "One Dark" scheme
/// (already tuned for legibility against a dark background); the
/// highlight backgrounds are darkened variants of the same hues, kept
/// deliberately subtle rather than vivid.
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

/// `DARK`'s opposite number, for a light terminal background — borrowed
/// from the equally well-known "One Light" scheme for the same reason:
/// already tuned for legibility, this time against a light background. A
/// near-white (not pure `#FFFFFF`, which reads as glaring) `bg` and
/// near-black `fg` — foreground accents are darker/more saturated than
/// `DARK`'s (needed for contrast against light rather than dark), and the
/// highlight backgrounds are pale tints rather than darkened shades.
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

/// Maximum distinction over subtlety: true black `bg`, true white `fg`,
/// fully saturated highlight backgrounds instead of `DARK`/`LIGHT`'s
/// deliberately muted ones, a brighter `dim` (still clearly de-emphasized,
/// just not faint), and vivid foreground accents. `derived_bg` is a vivid
/// amber/orange, clearly off `assertion_bg`'s red — red is reserved for
/// assertion alone here, same as in `DARK`/`LIGHT`.
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

/// `bg`/`fg`/`dim` are the same dark base as `DARK` — background doesn't
/// carry role information, so there's no colorblindness reason to change
/// it, and grey is colorblind-neutral already. The accents and highlight
/// backgrounds are the Okabe-Ito palette instead of `DARK`'s: sky blue
/// for `variable`, yellow for `label`, reddish purple for
/// `cursor_accent`, and — the three-way pairing that actually matters —
/// bluish green for `core_bg`, vermillion for `assertion_bg`, and yellow
/// for `derived_bg`, rather than `DARK`'s green/red/amber (red-vs-green
/// being the single hardest pairing under red-green color blindness, and
/// amber-vs-vermillion not reliably distinguishable either). `derived_bg`
/// reuses the same yellow hue as `label` — a background tint and a text
/// accent don't collide just for sharing a hue family. Highlight
/// backgrounds are darkened the same proportional way `DARK`'s are (each
/// channel scaled by ~0.35 off the canonical Okabe-Ito value), kept muted
/// rather than vivid.
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
    // Okabe-Ito vermillion vs bluish green: the palette's own pairing
    // for "bad"/"good" that survives red-green color blindness.
    error_fg: rgb(0xD5, 0x5E, 0x00),
    ok_fg: rgb(0x00, 0x9E, 0x73),
};

/// Which palette is active. `:theme <name>` switches it (TUI only — the
/// plain frontend has no color to theme in the first place).
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

    /// Exact-name lookup (case-sensitive, matching every other name-based
    /// lookup in this REPL — commands, labels, rules).
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
