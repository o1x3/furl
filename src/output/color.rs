//! ANSI color highlighting for HTTP messages (the `colors` output group).
//!
//! This is a clean-room reimplementation of the reference tool's coloring
//! stage, derived by probing the reference's *observable* byte output rather
//! than reading its source. The goal is byte-for-byte parity with the
//! reference when its colorized output is piped, for the two paths that
//! matter most:
//!
//! 1. The **generic 8/16-color** path (`--style=auto`, or any non-256-color
//!    terminal). This is the default and the most portable, so it is held to
//!    strict byte parity. It mirrors pygments' `TerminalFormatter`.
//! 2. The **pie 256-color** path (`--style=pie`/`pie-dark`/`pie-light` on a
//!    256-color terminal). Also held to strict byte parity. It mirrors
//!    pygments' `Terminal256Formatter` fed the HTTPie pie style sheets.
//!
//! Named non-pie 256-color styles (monokai, solarized, ...) are *not* pursued
//! to byte parity here: reproducing every pygments style sheet is out of
//! scope. They fall back to the bundled Solarized-ish 256 palette as a
//! documented approximation (see [`resolve_style`]).
//!
//! # Why re-tokenize instead of running a real lexer
//!
//! The head (status/request line + headers) has a rigid grammar, so we parse
//! it structurally rather than lex it — this is both simpler and exactly
//! reproduces the reference's token boundaries. Bodies need a JSON lexer;
//! we ship a small one ([`json_tokens`]) whose token stream matches
//! pygments' `JsonLexer` for the inputs the reference actually colorizes.
//!
//! # The two emitters
//!
//! The single most fiddly detail is how each pygments terminal formatter
//! breaks a colored token across newlines. The two formatters differ, and
//! the difference is load-bearing for reformatted (multi-line) bodies:
//!
//! - 8-color ([`emit_8`]): every `\n`-separated piece of a token — *including
//!   empty pieces* — is wrapped `START piece END`, joined by a bare `\n`.
//! - 256-color ([`emit_256`]): only *non-empty* pieces are wrapped; empty
//!   pieces (e.g. the run before a leading `\n`) contribute nothing.
//!
//! Both rules were pinned by feeding multi-line whitespace/string tokens to
//! the reference formatters and reading back the exact bytes.

// Body coloring lexes already-formatted body *text* directly rather than
// round-tripping through `crate::json`: the format stage has already produced
// the exact bytes (compact or reindented) we must colorize, so re-serializing
// would risk diverging from that text. The `crate::json` types are therefore
// not needed here.

/// The color capability of the output terminal, as probed at startup.
///
/// This drives whether coloring happens at all and, when it does, whether a
/// requested 256-color style is honored or downgraded to the generic 8-color
/// ANSI palette (spec §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// The terminal reports zero colors: the colors formatter disables itself
    /// entirely, emitting no escapes even under `--pretty=colors`.
    None,
    /// Basic 8/16-color ANSI. Requested 256-color styles are ignored and the
    /// generic terminal-default palette is used instead.
    Ansi8,
    /// Full 256-color ANSI: named/pie styles are honored via `38;5;N` escapes.
    Ansi256,
}

/// Probe the terminal color depth from `$TERM` (spec §3.4).
///
/// The reference queries the terminfo database (`tigetnum("colors")` after
/// `setupterm`) and maps: exactly `256` → 256-color; `0` → disabled; probe
/// failure → assume 256; anything else (8, 16, `-1` for an absent capability)
/// → downgrade to the basic 8-color formatter.
///
/// We do not link a terminfo library (it would be a new dependency), so we
/// approximate that mapping from the `$TERM` name, which covers the cases the
/// reference's own test suite exercises:
///
/// - unset/empty `$TERM` (equivalent to `setupterm` failing) → [`Ansi256`].
/// - a name advertising 256 colors (`*-256color`, or containing `256color`)
///   → [`Ansi256`].
/// - everything else → [`Ansi8`] (matches the reference emitting 8-color for
///   `xterm`, `linux`, `dumb`, `vt100`, ...).
///
/// This never returns [`None`] from `$TERM` alone, matching the reference's
/// observed behavior (even `dumb` produced 8-color output). [`None`] remains
/// representable so callers and tests can force the disabled path.
///
/// [`Ansi256`]: ColorDepth::Ansi256
/// [`Ansi8`]: ColorDepth::Ansi8
/// [`None`]: ColorDepth::None
pub fn detect_color_depth() -> ColorDepth {
    match std::env::var("TERM") {
        Ok(term) if !term.is_empty() => {
            if term.contains("256color") || term.ends_with("-256") {
                ColorDepth::Ansi256
            } else {
                ColorDepth::Ansi8
            }
        }
        // Unset or empty: the reference's setupterm fails and it assumes 256.
        _ => ColorDepth::Ansi256,
    }
}

/// Are colors active for this depth? False only when the terminal reports
/// zero colors (spec §3.4: `colors == 0` disables the formatter). The caller
/// uses this to decide whether to run the colors stage at all.
pub fn colors_active(depth: ColorDepth) -> bool {
    !matches!(depth, ColorDepth::None)
}

// ---------------------------------------------------------------------------
// Escape-sequence primitives
// ---------------------------------------------------------------------------

/// One resolved terminal color: the exact `start` escape that opens it and
/// the exact `reset` escape that closes it, plus whether it is "empty" (no
/// styling at all, e.g. an uncolored punctuation/operator token).
///
/// Storing the literal escape strings — rather than a semantic description —
/// keeps the emitters trivial and guarantees byte parity: the strings are
/// exactly what the reference formatters produce for the corresponding style.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Color {
    start: String,
    reset: String,
}

impl Color {
    /// A color that applies no styling: tokens carrying it are emitted as
    /// their raw text (used for punctuation, operators, and plain text that
    /// the reference leaves uncolored).
    fn none() -> Color {
        Color {
            start: String::new(),
            reset: String::new(),
        }
    }
}

/// Build an 8-color (generic `TerminalFormatter`) color from its raw SGR
/// prefix codes. The reset is always `\x1b[39;49;00m`, matching the
/// reference: it resets foreground, background, and all attributes at once.
///
/// `codes` are the CSI bodies in emission order — e.g. `["04", "36"]` yields
/// the underline-then-cyan `\x1b[04m\x1b[36m` prefix the reference uses for a
/// URL path.
fn c8(codes: &[&str]) -> Color {
    let start = codes.iter().map(|code| format!("\x1b[{code}m")).collect();
    Color {
        start,
        reset: "\x1b[39;49;00m".to_string(),
    }
}

/// Build a 256-color (`Terminal256Formatter`) color for a foreground palette
/// index, optionally bold. Matches the reference's `EscapeSequence`:
/// `\x1b[38;5;N[;01]m` to open, `\x1b[39m` (or `\x1b[39;00m` when bold, since
/// bold is an attribute that must also be reset) to close.
fn c256(index: u8, bold: bool) -> Color {
    if bold {
        Color {
            start: format!("\x1b[38;5;{index};01m"),
            reset: "\x1b[39;00m".to_string(),
        }
    } else {
        Color {
            start: format!("\x1b[38;5;{index}m"),
            reset: "\x1b[39m".to_string(),
        }
    }
}

/// Build a 256-formatter color that refers to an ANSI base color *by name*
/// rather than by cube index. The reference emits these as the aixterm SGR
/// (`\x1b[90m` for bright-black), not as `38;5;8`. This is how the pie styles
/// render "primary" text (header values, URL path) at shade 600.
fn c256_ansi_bright_black(bold: bool) -> Color {
    if bold {
        Color {
            start: "\x1b[90;01m".to_string(),
            reset: "\x1b[39;00m".to_string(),
        }
    } else {
        Color {
            start: "\x1b[90m".to_string(),
            reset: "\x1b[39m".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// The resolved style
// ---------------------------------------------------------------------------

/// Which formatter family a resolved style emits with. The two families have
/// different newline-splitting rules (see the module docs), so the emitter
/// must know which one it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// Generic 8/16-color ANSI (`auto`, or any downgrade).
    Ansi8,
    /// 256-color ANSI (pie or named styles).
    Ansi256,
}

/// A fully resolved coloring style: every semantic token already mapped to
/// its exact terminal escape, plus the formatter family that governs how
/// multi-line tokens are split.
///
/// Resolving up front keeps the hot path (colorizing a message) branch-light
/// and makes the escape strings testable in isolation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Style {
    family: Family,

    // --- Head: status / request line ---
    /// `HTTP` keyword and the version number.
    proto: Color,
    /// The `/` in `HTTP/1.1`. The generic HTTP lexer tokenizes it as an
    /// Operator (uncolored); the pie precise lexer colors it grey-bold. It is
    /// therefore a distinct color from [`proto`].
    ///
    /// [`proto`]: Style::proto
    proto_slash: Color,
    /// The `GET`/`POST`/... method (per-method in pie, uniform in 8-color).
    method_get: Color,
    method_post: Color,
    method_put: Color,
    method_delete: Color,
    method_other: Color,
    /// The request URL path.
    url: Color,
    /// Status code + reason phrase, by class.
    status_1xx: Color,
    status_2xx: Color,
    status_3xx: Color,
    status_4xx: Color,
    status_5xx: Color,
    /// The reason phrase. In the generic lexer this is a *different* token
    /// from the status code (cyan, not the code's blue); in pie the reason
    /// shares the status-class color, so this holds that class color there.
    reason: Color,

    // --- Head: header lines ---
    header_name: Color,
    header_value: Color,
    /// The `:` separating a header name from its value (colored only in pie).
    header_colon: Color,

    // --- Meta ---
    meta_key: Color,
    meta_colon: Color,
    meta_fast: Color,
    meta_avg: Color,
    meta_slow: Color,
    meta_very_slow: Color,
    /// The unit token (`s`) after the elapsed-time number.
    meta_unit: Color,

    // --- Body (JSON) ---
    json_key: Color,
    json_string: Color,
    json_number: Color,
    json_keyword: Color,
    json_punct: Color,
    json_ws: Color,
    /// The XSSI/anti-hijacking prefix (`Token.Error`) the enhanced JSON lexer
    /// emits. Underline bright-red in the generic style; primary text in pie.
    json_error: Color,
}

/// Resolve a `--style` name against a probed color depth into a [`Style`]
/// (spec §3.2–§3.4).
///
/// Resolution rules:
/// - `auto`, or *any* depth that is not 256-color, yields the generic 8-color
///   style (terminal-default palette). This is the reference's downgrade
///   behavior: a requested 256 style is ignored on an 8-color terminal.
/// - `pie` / `pie-dark` / `pie-light` on a 256-color terminal yield the pie
///   256 style at shade 600 / 500 / 700 respectively.
/// - Any other named style on a 256-color terminal is *approximated* by the
///   pie shade-600 palette. Byte parity with arbitrary pygments styles is out
///   of scope; this keeps such styles colored and legible without pretending
///   to match the reference byte-for-byte.
///
/// A [`ColorDepth::None`] depth still resolves to a style (the generic one),
/// but the caller is expected to gate on [`colors_active`] first and skip
/// coloring entirely.
pub fn resolve_style(style_name: &str, depth: ColorDepth) -> Style {
    // Auto, or any non-256 depth, is the generic path — the requested style
    // name is deliberately ignored here (§3.2, §3.4).
    if style_name == "auto" || depth != ColorDepth::Ansi256 {
        return generic_style();
    }
    match style_name {
        "pie-dark" => pie_style(Shade::S500),
        "pie-light" => pie_style(Shade::S700),
        // `pie`/`pie-universal` and any other named style map to shade 600.
        // Named non-pie styles are an intentional approximation (see docs).
        _ => pie_style(Shade::S600),
    }
}

/// The generic 8/16-color style used by `--style=auto` and every downgrade.
///
/// Every escape here was read back from the reference's `TerminalFormatter`
/// for the corresponding pygments token, so the strings are exact:
///
/// | token                | escape prefix     |
/// |----------------------|-------------------|
/// | `HTTP`/ver/status/num| `\x1b[34m` (blue) |
/// | reason/header name   | `\x1b[36m` (cyan) |
/// | method (Name.Function)| `\x1b[32m` (green)|
/// | URL (Name.Namespace) | `\x1b[04m\x1b[36m`|
/// | meta key (Decorator) | `\x1b[90m`        |
/// | meta unit (Builtin)  | `\x1b[36m`        |
/// | JSON key (Name.Tag)  | `\x1b[94m`        |
/// | JSON string          | `\x1b[33m`        |
/// | JSON number/keyword  | `\x1b[34m`        |
/// | whitespace (Text.WS) | `\x1b[37m`        |
/// | punctuation/operator | *(uncolored)*     |
fn generic_style() -> Style {
    let blue = c8(&["34"]);
    let cyan = c8(&["36"]);
    let green = c8(&["32"]);
    let bright_blue = c8(&["94"]);
    let yellow = c8(&["33"]);
    let bright_black = c8(&["90"]);
    let white = c8(&["37"]);
    let url = c8(&["04", "36"]);
    Style {
        family: Family::Ansi8,
        proto: blue.clone(),
        // The `/` is an Operator in the generic lexer → uncolored.
        proto_slash: Color::none(),
        // The generic HTTP lexer is non-"precise": all methods collapse to
        // Name.Function → green. Per-method coloring only exists in pie.
        method_get: green.clone(),
        method_post: green.clone(),
        method_put: green.clone(),
        method_delete: green.clone(),
        method_other: green,
        url,
        // All status classes are just Number → blue in the generic lexer.
        status_1xx: blue.clone(),
        status_2xx: blue.clone(),
        status_3xx: blue.clone(),
        status_4xx: blue.clone(),
        status_5xx: blue.clone(),
        // The reason phrase is a distinct cyan token in the generic lexer.
        reason: cyan.clone(),
        header_name: cyan.clone(),
        header_value: Color::none(),
        header_colon: Color::none(),
        meta_key: bright_black,
        meta_colon: Color::none(),
        // The generic metadata lexer colors the whole timing value as Number
        // regardless of speed; only pie has fine-grained speed tokens.
        meta_fast: blue.clone(),
        meta_avg: blue.clone(),
        meta_slow: blue.clone(),
        meta_very_slow: blue.clone(),
        meta_unit: cyan.clone(),
        json_key: bright_blue,
        json_string: yellow,
        json_number: blue.clone(),
        json_keyword: blue,
        json_punct: Color::none(),
        json_ws: white,
        // Token.Error: underline + bright-red in the generic lexer.
        json_error: c8(&["04", "91"]),
    }
}

/// The three pie shades the message palette consumes (spec §3.3): `pie-dark`
/// → 500, `pie` (universal) → 600, `pie-light` → 700.
#[derive(Debug, Clone, Copy)]
enum Shade {
    S500,
    S600,
    S700,
}

/// A single pie color as a `(hex-500, hex-600, hex-700)` triple. Only these
/// three shades feed message coloring; the full 10-stop scale in the spec is
/// not needed here. Hex strings are the brand palette reproduced as data.
struct PieColor {
    s500: &'static str,
    s600: &'static str,
    s700: &'static str,
}

impl PieColor {
    fn hex(&self, shade: Shade) -> &'static str {
        match shade {
            Shade::S500 => self.s500,
            Shade::S600 => self.s600,
            Shade::S700 => self.s700,
        }
    }
}

// The pie brand palette (spec §3.3), shades 500/600/700 only, reproduced as
// data. GREY is omitted here because the terminal override collapses it to a
// single ANSI-name color; see `pie_style`.
const GREEN: PieColor = PieColor {
    s500: "73DC8C",
    s600: "63C27A",
    s700: "52AB66",
};
const YELLOW: PieColor = PieColor {
    s500: "DBDE52",
    s600: "CCCC3D",
    s700: "BABA29",
};
const ORANGE: PieColor = PieColor {
    s500: "FFA24E",
    s600: "F2913D",
    s700: "E3822B",
};
const RED: PieColor = PieColor {
    s500: "FF665B",
    s600: "E34F45",
    s700: "C7382E",
};
const BLUE: PieColor = PieColor {
    s500: "4B78E6",
    s600: "426BD1",
    s700: "3B5EBA",
};
const PINK: PieColor = PieColor {
    s500: "FA9BFA",
    s600: "DE85DE",
    s700: "C26EC2",
};
const AQUA: PieColor = PieColor {
    s500: "8CB4CD",
    s600: "7A9EB5",
    s700: "698799",
};

/// Build the pie 256-color style for one shade.
///
/// Per spec §3.3 terminal overrides:
/// - GREY collapses to `#7D7D7D` at every shade → cube index 8. It carries
///   the `HTTP`/`/`/version, all punctuation, header colon, meta key, and the
///   JSON punctuation.
/// - PRIMARY (main text: header values, URL path) is shade-specific: 700 →
///   `#1C1818` (black, index 234), 600 → ANSI bright-black (`\x1b[90m`), 500 →
///   `#F5F5F0` (white, index 255).
/// - All other palette colors map by nearest xterm-256 index (see
///   [`nearest_256`]).
///
/// The header sheet bolds the status/method/proto tokens; the body sheet does
/// not bold. These bold flags were confirmed against probed output.
fn pie_style(shade: Shade) -> Style {
    // GREY, shade-independent, → cube index 8.
    let grey = nearest_256("7D7D7D");
    let grey_plain = c256(grey, false);
    let grey_bold = c256(grey, true);

    // PRIMARY per shade.
    let primary_plain = match shade {
        Shade::S500 => c256(nearest_256("F5F5F0"), false),
        Shade::S600 => c256_ansi_bright_black(false),
        Shade::S700 => c256(nearest_256("1C1818"), false),
    };
    let primary_bold = match shade {
        Shade::S500 => c256(nearest_256("F5F5F0"), true),
        Shade::S600 => c256_ansi_bright_black(true),
        Shade::S700 => c256(nearest_256("1C1818"), true),
    };

    let col = |pc: &PieColor, bold: bool| c256(nearest_256(pc.hex(shade)), bold);

    Style {
        family: Family::Ansi256,
        proto: grey_bold.clone(),
        // The `/` is grey-bold in the pie precise lexer.
        proto_slash: grey_bold.clone(),
        method_get: col(&GREEN, true),
        method_post: col(&YELLOW, true),
        method_put: col(&ORANGE, true),
        method_delete: col(&RED, true),
        method_other: grey_bold.clone(),
        url: primary_bold,
        status_1xx: col(&AQUA, true),
        status_2xx: col(&GREEN, true),
        status_3xx: col(&YELLOW, true),
        status_4xx: col(&ORANGE, true),
        status_5xx: col(&RED, true),
        // In pie the reason shares the status-class color, chosen per-response
        // in `colorize_status_line`; this field is unused for pie (the status
        // line consults `status_color` directly when the family is 256).
        reason: grey_plain.clone(),
        header_name: col(&BLUE, false),
        header_value: primary_plain.clone(),
        header_colon: grey_bold.clone(),
        meta_key: grey_plain.clone(),
        meta_colon: grey_bold,
        // Speed classes reuse the status class colors, bold (§9).
        meta_fast: col(&GREEN, true),
        meta_avg: col(&YELLOW, true),
        meta_slow: col(&ORANGE, true),
        meta_very_slow: col(&RED, true),
        // The `s` unit is uncolored in pie (confirmed against probed output).
        meta_unit: Color::none(),
        json_key: col(&PINK, false),
        json_string: col(&GREEN, false),
        json_number: col(&AQUA, false),
        json_keyword: col(&ORANGE, false),
        json_punct: grey_plain,
        // Whitespace and the XSSI Error prefix are both PRIMARY in the body
        // sheet (confirmed against probed output: the prefix renders as
        // `\x1b[90m` at shade 600, i.e. the same primary color).
        json_ws: primary_plain.clone(),
        json_error: primary_plain,
    }
}

/// Map a `RRGGBB` hex color to the nearest xterm-256 palette index by squared
/// Euclidean distance in RGB, over the full 256-entry table the reference's
/// pygments `Terminal256Formatter` uses:
///
/// - indices 0–15: the fixed xterm base colors (specific RGBs, *not* a formula);
/// - indices 16–231: the 6×6×6 color cube over levels `{0,95,135,175,215,255}`;
/// - indices 232–255: the 24-step grayscale ramp `8 + i*10`.
///
/// This exact table and metric were validated to reproduce every pie shade
/// index observed in the reference's output (e.g. `#63C27A`→72, `#7D7D7D`→8,
/// `#426BD1`→62).
fn nearest_256(hex: &str) -> u8 {
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0) as i32;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0) as i32;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0) as i32;

    let mut best = 0u8;
    let mut best_dist = i32::MAX;
    for (index, (cr, cg, cb)) in palette_256().into_iter().enumerate() {
        let dr = cr as i32 - r;
        let dg = cg as i32 - g;
        let db = cb as i32 - b;
        let dist = dr * dr + dg * dg + db * db;
        if dist < best_dist {
            best_dist = dist;
            best = index as u8;
        }
    }
    best
}

/// The full 256-entry xterm palette used for nearest-color matching.
fn palette_256() -> Vec<(u8, u8, u8)> {
    // The 16 fixed base colors (as pygments' table defines them).
    let base16: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    let mut table: Vec<(u8, u8, u8)> = base16.to_vec();
    // The 6×6×6 cube.
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    for &r in &LEVELS {
        for &g in &LEVELS {
            for &b in &LEVELS {
                table.push((r, g, b));
            }
        }
    }
    // The 24-step grayscale ramp.
    for i in 0..24u16 {
        let level = (8 + i * 10) as u8;
        table.push((level, level, level));
    }
    table
}

// ---------------------------------------------------------------------------
// Emitters — the newline-splitting rules that pin byte parity
// ---------------------------------------------------------------------------

/// Split a token value into its newline-terminated lines: each item is a
/// `(content, terminated)` pair where `content` is the text up to (but not
/// including) a `\n`, and `terminated` says whether that `\n` was present.
///
/// This models how both pygments terminal formatters iterate a token: they
/// wrap the content of each line, then emit a bare `\n` after every terminated
/// line. `"\n"` yields one item `("", true)`; `"a\nb"` yields `("a", true),
/// ("b", false)`; `"a\nb\n"` yields `("a", true), ("b", true)`. A trailing
/// `\n` therefore does *not* produce an extra empty trailing line.
fn newline_lines(value: &str) -> Vec<(&str, bool)> {
    let mut lines = Vec::new();
    let mut rest = value;
    loop {
        match rest.find('\n') {
            Some(pos) => {
                lines.push((&rest[..pos], true));
                rest = &rest[pos + 1..];
            }
            None => {
                if !rest.is_empty() {
                    lines.push((rest, false));
                }
                break;
            }
        }
    }
    lines
}

/// Emit one colored token in the 8-color (`TerminalFormatter`) discipline.
///
/// For each newline-terminated line, the content — *including empty content* —
/// is wrapped `start content reset`, then a bare `\n` is emitted; a final
/// non-terminated remainder is wrapped without a trailing `\n`. An empty value
/// emits nothing. When the color is [`Color::none`], `start`/`reset` are empty
/// so this degenerates to emitting the raw text.
fn emit_8(out: &mut String, color: &Color, value: &str) {
    for (content, terminated) in newline_lines(value) {
        out.push_str(&color.start);
        out.push_str(content);
        out.push_str(&color.reset);
        if terminated {
            out.push('\n');
        }
    }
}

/// Emit one colored token in the 256-color (`Terminal256Formatter`)
/// discipline.
///
/// Same line iteration as [`emit_8`], but only *non-empty* content is wrapped;
/// empty content contributes nothing (so a leading/blank/trailing `\n` stays
/// bare). The `\n` after each terminated line is still emitted.
fn emit_256(out: &mut String, color: &Color, value: &str) {
    for (content, terminated) in newline_lines(value) {
        if !content.is_empty() {
            out.push_str(&color.start);
            out.push_str(content);
            out.push_str(&color.reset);
        }
        if terminated {
            out.push('\n');
        }
    }
}

impl Style {
    /// Emit one colored token using this style's formatter family.
    fn emit(&self, out: &mut String, color: &Color, value: &str) {
        match self.family {
            Family::Ansi8 => emit_8(out, color, value),
            Family::Ansi256 => emit_256(out, color, value),
        }
    }

    /// Emit raw (uncolored) text — used for punctuation the reference leaves
    /// bare and for structural characters we insert verbatim. Routed through
    /// the family emitter with a [`Color::none`] so newline handling stays
    /// consistent (both families reduce to the raw text here).
    fn emit_raw(&self, out: &mut String, value: &str) {
        self.emit(out, &Color::none(), value);
    }
}

// ---------------------------------------------------------------------------
// Head colorizing (response + request status/header block)
// ---------------------------------------------------------------------------

/// Colorize a rendered response head — the status line plus header lines
/// (spec §3.3, §5.4). The input is the block produced by
/// `render_response_head`: the status line, then header lines joined by
/// `\r\n`, with no trailing separator.
///
/// The reference re-joins head lines with `\n` (not `\r\n`) in colorized
/// output and `.strip()`s the result; we match that, splitting the input on
/// `\r\n`, coloring each line, and joining with `\n`.
pub fn colorize_response_head(text: &str, style: &Style) -> String {
    let mut lines = text.split("\r\n");
    let mut out = String::with_capacity(text.len() * 2);

    if let Some(status) = lines.next() {
        colorize_status_line(&mut out, status, style);
    }
    for line in lines {
        out.push('\n');
        colorize_header_line(&mut out, line, style);
    }
    // The reference `.strip()`s the highlighted head block (§5.4).
    out.trim().to_string()
}

/// Colorize a rendered request head — the request line plus header lines
/// (spec §3.3). Same joining discipline as the response head.
pub fn colorize_request_head(text: &str, style: &Style) -> String {
    let mut lines = text.split("\r\n");
    let mut out = String::with_capacity(text.len() * 2);

    if let Some(request_line) = lines.next() {
        colorize_request_line(&mut out, request_line, style);
    }
    for line in lines {
        out.push('\n');
        colorize_header_line(&mut out, line, style);
    }
    // The reference `.strip()`s the highlighted head block (§5.4).
    out.trim().to_string()
}

/// Colorize a response status line: `HTTP/<version> <status> <reason>`.
///
/// `HTTP`, `/`, and the version each get the proto color (they are separate
/// tokens in the reference — three escapes, not one). The status code and the
/// reason phrase share the status-class color.
fn colorize_status_line(out: &mut String, line: &str, style: &Style) {
    // Split into at most three whitespace-separated fields: `HTTP/ver`,
    // `code`, and the rest (reason, which may itself contain spaces).
    let mut parts = line.splitn(3, ' ');
    let proto_field = parts.next().unwrap_or("");
    let code_field = parts.next();
    let reason_field = parts.next();

    emit_proto(out, proto_field, style);

    if let Some(code) = code_field {
        let status_color = status_color(code, style);
        out.push(' ');
        style.emit(out, status_color, code);
        if let Some(reason) = reason_field {
            out.push(' ');
            // The reason phrase: a distinct cyan token in the generic lexer,
            // but the same status-class color as the code in pie. An empty
            // reason emits nothing (the head is stripped afterwards, so the
            // trailing separator space is removed).
            let reason_color = match style.family {
                Family::Ansi8 => &style.reason,
                Family::Ansi256 => status_color,
            };
            style.emit(out, reason_color, reason);
        }
    }
}

/// Colorize a request line: `<METHOD> <path> HTTP/1.1`.
///
/// The method color is per-method in pie (uniform green in 8-color); the path
/// takes the URL color; `HTTP/1.1` is the proto triple.
fn colorize_request_line(out: &mut String, line: &str, style: &Style) {
    let mut parts = line.splitn(3, ' ');
    let method = parts.next().unwrap_or("");
    let path = parts.next();
    let proto = parts.next();

    style.emit(out, method_color(method, style), method);
    if let Some(path) = path {
        out.push(' ');
        style.emit(out, &style.url, path);
    }
    if let Some(proto) = proto {
        out.push(' ');
        emit_proto(out, proto, style);
    }
}

/// Emit an `HTTP/<version>` field as the reference's three separate proto
/// tokens: `HTTP`, the `/`, and the version number.
fn emit_proto(out: &mut String, field: &str, style: &Style) {
    match field.split_once('/') {
        Some((name, version)) => {
            style.emit(out, &style.proto, name);
            style.emit(out, &style.proto_slash, "/");
            style.emit(out, &style.proto, version);
        }
        // No slash (unexpected): color the whole field as proto.
        None => style.emit(out, &style.proto, field),
    }
}

/// Colorize one header line: `Name: value`. The name and value each get their
/// color; the `:` is a separate token (colored only in pie), and the space
/// after it is emitted raw.
fn colorize_header_line(out: &mut String, line: &str, style: &Style) {
    match line.split_once(':') {
        Some((name, rest)) => {
            style.emit(out, &style.header_name, name);
            style.emit(out, &style.header_colon, ":");
            // The reference renders `Name: value` with the value token
            // starting after the single separating space. Preserve the exact
            // whitespace between the colon and the value verbatim.
            let value = rest.strip_prefix(' ');
            match value {
                Some(value) => {
                    out.push(' ');
                    style.emit(out, &style.header_value, value);
                }
                None => style.emit(out, &style.header_value, rest),
            }
        }
        // A line without a colon (should not occur in a well-formed head) is
        // emitted raw so nothing is lost.
        None => style.emit_raw(out, line),
    }
}

/// The status-class color for a numeric status code. Non-numeric or
/// out-of-range codes fall back to the 2xx color (they never occur in
/// practice; this keeps the function total).
fn status_color<'a>(code: &str, style: &'a Style) -> &'a Color {
    match code.as_bytes().first() {
        Some(b'1') => &style.status_1xx,
        Some(b'2') => &style.status_2xx,
        Some(b'3') => &style.status_3xx,
        Some(b'4') => &style.status_4xx,
        Some(b'5') => &style.status_5xx,
        _ => &style.status_2xx,
    }
}

/// The method color for a request method name. In the generic style all of
/// these resolve to the same green; in pie they diverge per method (§3.3).
fn method_color<'a>(method: &str, style: &'a Style) -> &'a Color {
    match method {
        "GET" | "HEAD" => &style.method_get,
        "POST" => &style.method_post,
        "PUT" | "PATCH" => &style.method_put,
        "DELETE" => &style.method_delete,
        _ => &style.method_other,
    }
}

// ---------------------------------------------------------------------------
// Meta colorizing
// ---------------------------------------------------------------------------

/// Colorize the meta section — a single `Elapsed time: <seconds>s` line
/// (spec §9). The key is a decorator token, the `:` an operator, the numeric
/// value colored by speed class (pie only; a plain number otherwise), and the
/// `s` unit a separate token.
///
/// A trailing whitespace token is appended to mirror the reference (its
/// metadata lexer emits a final whitespace token, which the formatters wrap).
pub fn colorize_meta(text: &str, style: &Style) -> String {
    let mut out = String::with_capacity(text.len() * 2);

    // The meta block is a single `key: value` line today. Split on the first
    // `: ` so a value can't be confused for the separator. The reference's
    // metadata lexer tokenizes: key (Decorator), `:` (Operator), ` ` (Text),
    // number (Number/speed), `s` (Builtin), then a trailing `\n` (Whitespace)
    // from `ensure_nl` — which the surrounding `.strip()` removes.
    let (key, value) = match text.split_once(": ") {
        Some(pair) => pair,
        // No recognizable structure: emit as raw key text and bail.
        None => {
            style.emit(&mut out, &style.meta_key, text);
            return out;
        }
    };

    style.emit(&mut out, &style.meta_key, key);
    // The `:` is an Operator (uncolored in generic; grey-bold in pie), and the
    // separating space is a plain Text token (always uncolored).
    style.emit(&mut out, &style.meta_colon, ":");
    out.push(' ');

    // Split the value into the numeric part and the `s` unit. The value looks
    // like `<digits>.<digits>s`; the unit is the trailing non-digit suffix.
    let (number, unit) = split_elapsed(value);

    let speed_color = speed_color(number, style);
    style.emit(&mut out, speed_color, number);
    style.emit(&mut out, &style.meta_unit, unit);

    // The lexer's trailing `\n` whitespace token, then `.strip()` per §5.4.
    // In 8-color this leaves a bare `\x1b[37m\x1b[39;49;00m` after the strip
    // eats the newline; in 256 the empty-content line emits nothing.
    style.emit(&mut out, &style.json_ws, "\n");
    out.trim_end().to_string()
}

/// Split an elapsed-time value like `0.1013629169s` into `("0.1013629169",
/// "s")`. The unit is the trailing run of non-`[0-9.]` characters; if none,
/// the whole thing is the number and the unit is empty.
fn split_elapsed(value: &str) -> (&str, &str) {
    let split_at = value
        .char_indices()
        .find(|&(_, c)| !(c.is_ascii_digit() || c == '.'))
        .map(|(i, _)| i)
        .unwrap_or(value.len());
    value.split_at(split_at)
}

/// The speed-class color for an elapsed-time number (spec §9): ≤ 0.45s fast,
/// ≤ 1.0s average, ≤ 2.5s slow, else very slow. In the generic style all four
/// resolve to the same plain-number color, so the thresholds are moot there
/// but still computed uniformly.
fn speed_color<'a>(number: &str, style: &'a Style) -> &'a Color {
    let seconds: f64 = number.parse().unwrap_or(0.0);
    if seconds <= 0.45 {
        &style.meta_fast
    } else if seconds <= 1.0 {
        &style.meta_avg
    } else if seconds <= 2.5 {
        &style.meta_slow
    } else {
        &style.meta_very_slow
    }
}

// ---------------------------------------------------------------------------
// Body colorizing (JSON)
// ---------------------------------------------------------------------------

/// Whether the reference would select the JSON lexer for `mime` (§5.5).
///
/// The selection is driven by the *subtype*: the JSON lexer applies iff the
/// subtype contains the substring `json` (covering `json`, `x-json`,
/// `json-foo`, and any `+json`/`json+` composite subtype). The type half is
/// ignored so a hypothetical `json/plain` — subtype `plain`, no `json` — does
/// not falsely qualify. A MIME with no `/` is treated as a bare subtype.
fn json_lexer_applies(mime: &str) -> bool {
    let subtype = mime.split_once('/').map(|(_, sub)| sub).unwrap_or(mime);
    // Strip any `;parameters` the caller may have left on the media type.
    let subtype = subtype.split(';').next().unwrap_or(subtype);
    subtype.contains("json")
}

/// Colorize a message body (spec §5.4, §5.5).
///
/// The JSON lexer is selected when the effective MIME's *subtype* contains the
/// substring `json` (§5.5): the subtype itself, or — for a `+`-suffixed
/// subtype — its base or suffix resolves to the `json` lexer. This matches the
/// reference lexer-selection list (§11): `application/json`, `application/x-json`,
/// `application/json-foo`, `foo/json`, `foo/bar+json`, `application/hal+json`,
/// etc. all select the JSON lexer, while `text/plain`, `text/html`, and
/// `application/javascript` do NOT — the reference routes those to its own
/// plain-text / HTML / JavaScript lexers (a different or empty token stream),
/// which we do not reproduce, so we leave them uncolored rather than
/// mis-coloring them as JSON. Any MIME whose subtype has no `json` leaves the
/// body uncolored.
///
/// When coloring happens, a trailing `\n` is guaranteed on the output, matching
/// the reference's highlighter (§5.4).
///
/// The body text passed in is already format-stage output (compact under
/// `--pretty=colors`, reindented under `--pretty=all`); we only tokenize and
/// color it — we never reformat here.
pub fn colorize_body(text: &str, mime: Option<&str>, style: &Style) -> String {
    let json_eligible = mime.map(json_lexer_applies).unwrap_or(false);

    if !json_eligible {
        // No JSON lexer applies: pass the body through uncolored, unchanged.
        // (The reference may still color it with a non-JSON lexer; we only ship
        // a JSON lexer, so we decline rather than diverge.)
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len() * 2);
    // Adjacent tokens of the same kind are emitted as one colored run
    // (pygments coalesces same-type tokens — e.g. `},` is one escape span,
    // not two). Accumulate a same-kind run's text, then emit it once.
    let tokens = json_tokens(text);
    let mut i = 0;
    while i < tokens.len() {
        let kind = tokens[i].kind;
        let mut run = String::from(tokens[i].text);
        let mut j = i + 1;
        while j < tokens.len() && tokens[j].kind == kind {
            run.push_str(tokens[j].text);
            j += 1;
        }
        let color = match kind {
            JsonTok::Key => &style.json_key,
            JsonTok::String => &style.json_string,
            JsonTok::Number => &style.json_number,
            JsonTok::Keyword => &style.json_keyword,
            JsonTok::Punct => &style.json_punct,
            JsonTok::Whitespace => &style.json_ws,
            JsonTok::Error => &style.json_error,
        };
        style.emit(&mut out, color, &run);
        i = j;
    }

    // The highlighter guarantees a trailing newline on colored bodies (§5.4).
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// A JSON token kind, matching the pygments `JsonLexer` categories the
/// reference colors: object *keys* (`Name.Tag`) are distinguished from string
/// *values* (`String.Double`); numbers, keyword constants (`true`/`false`/
/// `null`), punctuation, and whitespace are the rest. `Error` covers an
/// XSSI/anti-hijacking prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonTok {
    Key,
    String,
    Number,
    Keyword,
    Punct,
    Whitespace,
    Error,
}

/// One lexed token: its kind and the exact source slice it spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Token<'a> {
    kind: JsonTok,
    text: &'a str,
}

/// Tokenize a JSON document into pygments-`JsonLexer`-compatible tokens.
///
/// The distinguishing rule the reference relies on: a string is an object
/// *key* iff the next non-whitespace character after it is `:`. Everything
/// else follows straightforwardly. Whitespace runs are coalesced into single
/// tokens (as pygments does), and a trailing `\n` is appended to the token
/// stream when the input does not already end in one (pygments' `ensure_nl`),
/// which is what produces the reference's trailing whitespace token.
///
/// This is deliberately lenient: malformed input still tokenizes (unmatched
/// quotes lex to end-of-input as a string, stray characters lex as
/// punctuation) so coloring never fails on a body the format stage left
/// as-is.
fn json_tokens(input: &str) -> Vec<Token<'_>> {
    let bytes = input.as_bytes();
    let mut tokens: Vec<Token> = Vec::new();
    let mut i = 0usize;

    // Track whether we are looking for the leading XSSI prefix. pygments'
    // enhanced JSON lexer tokenizes a non-JSON prefix (up to the first
    // `{`/`[`/`"`) as an Error token. We only do this at the very start.
    if let Some(prefix_len) = xssi_prefix_len(input) {
        tokens.push(Token {
            kind: JsonTok::Error,
            text: &input[..prefix_len],
        });
        i = prefix_len;
    }

    while i < bytes.len() {
        let b = bytes[i];
        match b {
            // Whitespace run.
            b' ' | b'\t' | b'\r' | b'\n' => {
                let start = i;
                while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
                    i += 1;
                }
                tokens.push(Token {
                    kind: JsonTok::Whitespace,
                    text: &input[start..i],
                });
            }
            // A string: a key if the next non-space char is `:`, else a value.
            b'"' => {
                let start = i;
                i = scan_string(bytes, i);
                let text = &input[start..i];
                let kind = if next_nonspace_is_colon(bytes, i) {
                    JsonTok::Key
                } else {
                    JsonTok::String
                };
                tokens.push(Token { kind, text });
            }
            // Punctuation / structural characters.
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                tokens.push(Token {
                    kind: JsonTok::Punct,
                    text: &input[i..i + 1],
                });
                i += 1;
            }
            // A number: optional sign, digits, `.`, exponent.
            b'-' | b'0'..=b'9' => {
                let start = i;
                i = scan_number(bytes, i);
                tokens.push(Token {
                    kind: JsonTok::Number,
                    text: &input[start..i],
                });
            }
            // A keyword constant: true / false / null.
            b't' | b'f' | b'n' => {
                if let Some(end) = scan_keyword(bytes, i) {
                    tokens.push(Token {
                        kind: JsonTok::Keyword,
                        text: &input[i..end],
                    });
                    i = end;
                } else {
                    // Not a recognized keyword: consume one byte as punct so
                    // we always make progress and never loop.
                    tokens.push(Token {
                        kind: JsonTok::Punct,
                        text: &input[i..i + 1],
                    });
                    i += 1;
                }
            }
            // Anything else: emit one byte's worth as punctuation to stay
            // total. (UTF-8 continuation bytes only occur inside strings, so
            // slicing one byte here is safe for the ASCII structural set.)
            _ => {
                let end = next_char_boundary(input, i);
                tokens.push(Token {
                    kind: JsonTok::Punct,
                    text: &input[i..end],
                });
                i = end;
            }
        }
    }

    // pygments' `ensure_nl`: append a trailing newline token if the input did
    // not already end in one. This is what yields the reference's final
    // whitespace token (e.g. `\x1b[37m\x1b[39;49;00m\n`).
    if !input.ends_with('\n') {
        // The appended newline is a distinct whitespace token with empty
        // preceding text in the compact case; the reference emits an empty
        // whitespace wrap then the raw `\n`. We model this as a whitespace
        // token whose text is "\n" only when the body has content, matching
        // the observed `...\x1b[37m\x1b[39;49;00m\n` shape after the 8-color
        // emitter splits it.
        tokens.push(Token {
            kind: JsonTok::Whitespace,
            text: "\n",
        });
    }

    tokens
}

/// The length of a leading XSSI/anti-hijacking prefix: the run of characters
/// before the first `{`, `[`, or `"`, but only when the body does not *start*
/// with one of those (a normal JSON body has no prefix). Returns `None` when
/// there is no prefix.
fn xssi_prefix_len(input: &str) -> Option<usize> {
    let first = input.as_bytes().first()?;
    if matches!(first, b'{' | b'[' | b'"') {
        return None;
    }
    // Also skip leading whitespace-only inputs — those are plain whitespace,
    // not an XSSI prefix.
    let prefix_len = input
        .char_indices()
        .take_while(|&(_, c)| c != '{' && c != '[' && c != '"')
        .map(|(idx, c)| idx + c.len_utf8())
        .last()
        .unwrap_or(0);
    // If the "prefix" is the entire input, there is no JSON after it — treat
    // the input as having no XSSI prefix so it lexes as plain tokens instead.
    if prefix_len == 0 || prefix_len >= input.len() {
        // Leading whitespace should still be a whitespace token, so only
        // claim a prefix when it contains a non-whitespace character.
        return None;
    }
    if input[..prefix_len].trim().is_empty() {
        // Pure whitespace before the JSON is whitespace, not an error prefix.
        return None;
    }
    Some(prefix_len)
}

/// Scan a JSON string starting at the opening quote; return the index just
/// past the closing quote (or end of input if unterminated). Handles `\"`
/// escapes.
fn scan_string(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1; // skip opening quote
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2, // skip escaped char
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// Whether the next non-space byte at/after `i` is a `:` (i.e. the preceding
/// string was an object key).
fn next_nonspace_is_colon(bytes: &[u8], i: usize) -> bool {
    let mut j = i;
    while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\r' | b'\n') {
        j += 1;
    }
    bytes.get(j) == Some(&b':')
}

/// Scan a JSON number token; return the index just past it.
fn scan_number(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    if i < bytes.len() && bytes[i] == b'-' {
        i += 1;
    }
    while i < bytes.len() && matches!(bytes[i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') {
        i += 1;
    }
    i.max(start + 1)
}

/// Scan a keyword constant (`true`/`false`/`null`) at `i`; return its end
/// index, or `None` if `i` does not begin one.
fn scan_keyword(bytes: &[u8], i: usize) -> Option<usize> {
    for kw in ["true", "false", "null"] {
        let end = i + kw.len();
        if bytes.len() >= end && &bytes[i..end] == kw.as_bytes() {
            return Some(end);
        }
    }
    None
}

/// The next UTF-8 char boundary strictly after `i`, so slicing `input[i..end]`
/// never splits a multi-byte character.
fn next_char_boundary(input: &str, i: usize) -> usize {
    let mut end = i + 1;
    while end < input.len() && !input.is_char_boundary(end) {
        end += 1;
    }
    end.min(input.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Color depth detection (§3.4) ---

    #[test]
    fn colors_active_false_only_for_none() {
        assert!(!colors_active(ColorDepth::None));
        assert!(colors_active(ColorDepth::Ansi8));
        assert!(colors_active(ColorDepth::Ansi256));
    }

    // --- nearest_256: validated against reference-probed indices ---

    #[test]
    fn nearest_256_reproduces_pie_600_indices() {
        assert_eq!(nearest_256("7D7D7D"), 8); // grey
        assert_eq!(nearest_256("63C27A"), 72); // green 600
        assert_eq!(nearest_256("CCCC3D"), 185); // yellow 600
        assert_eq!(nearest_256("F2913D"), 209); // orange 600
        assert_eq!(nearest_256("E34F45"), 167); // red 600
        assert_eq!(nearest_256("426BD1"), 62); // blue 600
        assert_eq!(nearest_256("DE85DE"), 176); // pink 600
        assert_eq!(nearest_256("7A9EB5"), 109); // aqua 600
    }

    #[test]
    fn nearest_256_reproduces_pie_500_and_700_indices() {
        // Shade 500.
        assert_eq!(nearest_256("73DC8C"), 78); // green 500
        assert_eq!(nearest_256("4B78E6"), 68); // blue 500
        assert_eq!(nearest_256("FFA24E"), 215); // orange 500
        assert_eq!(nearest_256("FA9BFA"), 213); // pink 500
        assert_eq!(nearest_256("8CB4CD"), 110); // aqua 500
        assert_eq!(nearest_256("F5F5F0"), 255); // primary 500 (white)
        // Shade 700.
        assert_eq!(nearest_256("52AB66"), 71); // green 700
        assert_eq!(nearest_256("3B5EBA"), 61); // blue 700
        assert_eq!(nearest_256("E3822B"), 172); // orange 700
        assert_eq!(nearest_256("C26EC2"), 133); // pink 700
        assert_eq!(nearest_256("698799"), 66); // aqua 700
        assert_eq!(nearest_256("1C1818"), 234); // primary 700 (black)
    }

    // --- Emitter newline rules (byte-exact vs reference formatters) ---

    #[test]
    fn emit_8_wraps_every_piece_including_empty() {
        // TerminalFormatter: '\n    ' with white → wrap empty, bare \n, wrap.
        let mut out = String::new();
        emit_8(&mut out, &c8(&["37"]), "\n    ");
        assert_eq!(out, "\x1b[37m\x1b[39;49;00m\n\x1b[37m    \x1b[39;49;00m");
    }

    #[test]
    fn emit_8_single_line() {
        let mut out = String::new();
        emit_8(&mut out, &c8(&["36"]), "Server");
        assert_eq!(out, "\x1b[36mServer\x1b[39;49;00m");
    }

    #[test]
    fn emit_8_empty_emits_nothing() {
        let mut out = String::new();
        emit_8(&mut out, &c8(&["37"]), "");
        assert_eq!(out, "");
    }

    #[test]
    fn emit_256_skips_empty_pieces() {
        // Terminal256Formatter: '\n    ' → bare \n then wrapped '    '.
        let mut out = String::new();
        emit_256(&mut out, &c256(250, false), "\n    ");
        assert_eq!(out, "\n\x1b[38;5;250m    \x1b[39m");
    }

    #[test]
    fn emit_256_bare_newline() {
        let mut out = String::new();
        emit_256(&mut out, &c256(250, false), "\n");
        assert_eq!(out, "\n");
    }

    // --- AUTO response head (byte-exact) ---

    #[test]
    fn auto_response_head_byte_exact() {
        let style = resolve_style("auto", ColorDepth::Ansi8);
        let head = "HTTP/1.1 200 OK\r\nServer: BaseHTTP/0.6\r\nContent-Type: application/json";
        let out = colorize_response_head(head, &style);
        let expected = concat!(
            "\x1b[34mHTTP\x1b[39;49;00m/\x1b[34m1.1\x1b[39;49;00m ",
            "\x1b[34m200\x1b[39;49;00m \x1b[36mOK\x1b[39;49;00m\n",
            "\x1b[36mServer\x1b[39;49;00m: BaseHTTP/0.6\n",
            "\x1b[36mContent-Type\x1b[39;49;00m: application/json",
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn auto_request_head_byte_exact() {
        let style = resolve_style("auto", ColorDepth::Ansi8);
        let head =
            "GET /get HTTP/1.1\r\nAccept-Encoding: gzip, deflate, zstd\r\nHost: 127.0.0.1:8099";
        let out = colorize_request_head(head, &style);
        let expected = concat!(
            "\x1b[32mGET\x1b[39;49;00m \x1b[04m\x1b[36m/get\x1b[39;49;00m ",
            "\x1b[34mHTTP\x1b[39;49;00m/\x1b[34m1.1\x1b[39;49;00m\n",
            "\x1b[36mAccept-Encoding\x1b[39;49;00m: gzip, deflate, zstd\n",
            "\x1b[36mHost\x1b[39;49;00m: 127.0.0.1:8099",
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn auto_status_all_classes_blue() {
        // In the generic style every status class is Number → blue.
        let style = resolve_style("auto", ColorDepth::Ansi8);
        for code in ["100", "204", "301", "404", "500"] {
            let head = format!("HTTP/1.1 {code} X");
            let out = colorize_response_head(&head, &style);
            assert!(
                out.contains(&format!("\x1b[34m{code}\x1b[39;49;00m")),
                "code {code}: {out:?}"
            );
        }
    }

    // --- PIE response head (byte-exact) ---

    #[test]
    fn pie_response_head_byte_exact() {
        let style = resolve_style("pie", ColorDepth::Ansi256);
        let head = "HTTP/1.1 200 OK\r\nServer: BaseHTTP/0.6\r\nContent-Type: application/json";
        let out = colorize_response_head(head, &style);
        let expected = concat!(
            "\x1b[38;5;8;01mHTTP\x1b[39;00m\x1b[38;5;8;01m/\x1b[39;00m\x1b[38;5;8;01m1.1\x1b[39;00m ",
            "\x1b[38;5;72;01m200\x1b[39;00m \x1b[38;5;72;01mOK\x1b[39;00m\n",
            "\x1b[38;5;62mServer\x1b[39m\x1b[38;5;8;01m:\x1b[39;00m \x1b[90mBaseHTTP/0.6\x1b[39m\n",
            "\x1b[38;5;62mContent-Type\x1b[39m\x1b[38;5;8;01m:\x1b[39;00m \x1b[90mapplication/json\x1b[39m",
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn pie_status_classes_byte_exact() {
        let style = resolve_style("pie", ColorDepth::Ansi256);
        let cases = [
            ("204", "\x1b[38;5;72;01m204\x1b[39;00m"),
            ("301", "\x1b[38;5;185;01m301\x1b[39;00m"),
            ("404", "\x1b[38;5;209;01m404\x1b[39;00m"),
            ("500", "\x1b[38;5;167;01m500\x1b[39;00m"),
        ];
        for (code, want) in cases {
            let head = format!("HTTP/1.1 {code} Custom");
            let out = colorize_response_head(&head, &style);
            assert!(out.contains(want), "code {code}: {out:?}");
        }
    }

    #[test]
    fn pie_request_line_methods_byte_exact() {
        let style = resolve_style("pie", ColorDepth::Ansi256);
        let cases = [
            ("GET", "\x1b[38;5;72;01mGET\x1b[39;00m"),
            ("POST", "\x1b[38;5;185;01mPOST\x1b[39;00m"),
            ("PUT", "\x1b[38;5;209;01mPUT\x1b[39;00m"),
            ("PATCH", "\x1b[38;5;209;01mPATCH\x1b[39;00m"),
            ("DELETE", "\x1b[38;5;167;01mDELETE\x1b[39;00m"),
            ("HEAD", "\x1b[38;5;72;01mHEAD\x1b[39;00m"),
        ];
        for (method, want) in cases {
            let head = format!("{method} /get HTTP/1.1");
            let out = colorize_request_head(&head, &style);
            assert!(out.starts_with(want), "method {method}: {out:?}");
            // URL path is primary-bold bright-black in pie.
            assert!(out.contains("\x1b[90;01m/get\x1b[39;00m"), "{out:?}");
        }
    }

    // --- AUTO JSON body (byte-exact) ---

    #[test]
    fn auto_json_body_compact_byte_exact() {
        let style = resolve_style("auto", ColorDepth::Ansi8);
        let body = r#"{"n":42,"t":true,"nil":null,"s":"hi"}"#;
        let out = colorize_body(body, Some("application/json"), &style);
        let expected = concat!(
            "{\x1b[94m\"n\"\x1b[39;49;00m:\x1b[34m42\x1b[39;49;00m,",
            "\x1b[94m\"t\"\x1b[39;49;00m:\x1b[34mtrue\x1b[39;49;00m,",
            "\x1b[94m\"nil\"\x1b[39;49;00m:\x1b[34mnull\x1b[39;49;00m,",
            "\x1b[94m\"s\"\x1b[39;49;00m:\x1b[33m\"hi\"\x1b[39;49;00m}",
            "\x1b[37m\x1b[39;49;00m\n",
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn auto_json_body_reindented_byte_exact() {
        // A reindented body exercises the multi-line whitespace token split.
        let style = resolve_style("auto", ColorDepth::Ansi8);
        let body = "{\n    \"a\": 1\n}";
        let out = colorize_body(body, Some("application/json"), &style);
        let expected = concat!(
            "{\x1b[37m\x1b[39;49;00m\n\x1b[37m    \x1b[39;49;00m",
            "\x1b[94m\"a\"\x1b[39;49;00m:\x1b[37m \x1b[39;49;00m\x1b[34m1\x1b[39;49;00m",
            "\x1b[37m\x1b[39;49;00m\n}\x1b[37m\x1b[39;49;00m\n",
        );
        assert_eq!(out, expected);
    }

    // --- PIE JSON body (byte-exact) ---

    #[test]
    fn pie_json_body_compact_byte_exact() {
        let style = resolve_style("pie", ColorDepth::Ansi256);
        let body = r#"{"n":42,"t":true,"nil":null,"s":"hi"}"#;
        let out = colorize_body(body, Some("application/json"), &style);
        let expected = concat!(
            "\x1b[38;5;8m{\x1b[39m\x1b[38;5;176m\"n\"\x1b[39m\x1b[38;5;8m:\x1b[39m",
            "\x1b[38;5;109m42\x1b[39m\x1b[38;5;8m,\x1b[39m\x1b[38;5;176m\"t\"\x1b[39m",
            "\x1b[38;5;8m:\x1b[39m\x1b[38;5;209mtrue\x1b[39m\x1b[38;5;8m,\x1b[39m",
            "\x1b[38;5;176m\"nil\"\x1b[39m\x1b[38;5;8m:\x1b[39m\x1b[38;5;209mnull\x1b[39m",
            "\x1b[38;5;8m,\x1b[39m\x1b[38;5;176m\"s\"\x1b[39m\x1b[38;5;8m:\x1b[39m",
            "\x1b[38;5;72m\"hi\"\x1b[39m\x1b[38;5;8m}\x1b[39m\n",
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn pie_json_body_reindented_whitespace_byte_exact() {
        // Multi-line body: the 256 emitter leaves leading \n bare and wraps
        // the indentation in primary; the trailing } newline stays bare.
        let style = resolve_style("pie", ColorDepth::Ansi256);
        let body = "{\n    \"a\": 1\n}";
        let out = colorize_body(body, Some("application/json"), &style);
        let expected = concat!(
            "\x1b[38;5;8m{\x1b[39m\n\x1b[90m    \x1b[39m",
            "\x1b[38;5;176m\"a\"\x1b[39m\x1b[38;5;8m:\x1b[39m\x1b[90m \x1b[39m",
            "\x1b[38;5;109m1\x1b[39m\n\x1b[38;5;8m}\x1b[39m\n",
        );
        assert_eq!(out, expected);
    }

    // --- Body lexer eligibility (§5.5) ---

    #[test]
    fn body_non_json_mime_uncolored() {
        let style = resolve_style("auto", ColorDepth::Ansi8);
        let body = r#"{"a":1}"#;
        let out = colorize_body(body, Some("application/octet-stream"), &style);
        assert_eq!(out, body, "non-json mime must pass through uncolored");
    }

    #[test]
    fn body_json_subtype_mimes_are_json_eligible() {
        // Only subtypes containing `json` select the JSON lexer (§5.5, §11).
        let style = resolve_style("auto", ColorDepth::Ansi8);
        for mime in [
            "application/json",
            "application/vnd.api+json",
            "application/x-json",
            "application/json-foo",
            "foo/json",
            "foo/bar+json",
            "application/hal+json",
        ] {
            let out = colorize_body(r#"{"a":1}"#, Some(mime), &style);
            assert!(
                out.contains("\x1b["),
                "mime {mime} should colorize: {out:?}"
            );
        }
    }

    #[test]
    fn body_non_json_lexer_mimes_pass_through_uncolored() {
        // The reference routes these to its plain-text / HTML / JavaScript
        // lexers (verified by probing httpie 3.2.4 with --response-mime): a
        // JSON body under `text/plain`, `text/html`, or `application/javascript`
        // is NOT colored as JSON. We ship only a JSON lexer, so we leave them
        // uncolored rather than mis-coloring them as JSON.
        let style = resolve_style("auto", ColorDepth::Ansi8);
        let body = r#"{"a":1}"#;
        for mime in [
            "text/plain",
            "text/html",
            "application/javascript",
            "application/xml",
            "application/octet-stream",
        ] {
            let out = colorize_body(body, Some(mime), &style);
            assert_eq!(out, body, "mime {mime} must pass through uncolored");
        }
    }

    #[test]
    fn body_none_mime_uncolored() {
        let style = resolve_style("auto", ColorDepth::Ansi8);
        assert_eq!(colorize_body("{}", None, &style), "{}");
    }

    #[test]
    fn body_trailing_newline_guaranteed() {
        let style = resolve_style("auto", ColorDepth::Ansi8);
        let out = colorize_body("{}", Some("application/json"), &style);
        assert!(out.ends_with('\n'));
    }

    // --- XSSI prefix ---

    #[test]
    fn body_xssi_prefix_lexed_as_error_auto_byte_exact() {
        // The XSSI prefix `)]}',\n` is a single Error token (underline
        // bright-red in the generic lexer); its trailing `\n` is bare, then
        // the JSON object colorizes normally.
        let style = resolve_style("auto", ColorDepth::Ansi8);
        let out = colorize_body(")]}',\n{\"a\":1}", Some("application/json"), &style);
        let expected = concat!(
            "\x1b[04m\x1b[91m)]}',\x1b[39;49;00m\n",
            "{\x1b[94m\"a\"\x1b[39;49;00m:\x1b[34m1\x1b[39;49;00m}",
            "\x1b[37m\x1b[39;49;00m\n",
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn body_xssi_prefix_lexed_as_error_pie_byte_exact() {
        // In pie the Error prefix is PRIMARY (bright-black); trailing `\n` bare.
        let style = resolve_style("pie", ColorDepth::Ansi256);
        let out = colorize_body(")]}',\n{\"a\":1}", Some("application/json"), &style);
        let expected = concat!(
            "\x1b[90m)]}',\x1b[39m\n",
            "\x1b[38;5;8m{\x1b[39m\x1b[38;5;176m\"a\"\x1b[39m\x1b[38;5;8m:\x1b[39m",
            "\x1b[38;5;109m1\x1b[39m\x1b[38;5;8m}\x1b[39m\n",
        );
        assert_eq!(out, expected);
    }

    // --- AUTO meta (byte-exact) ---

    #[test]
    fn auto_meta_byte_exact() {
        let style = resolve_style("auto", ColorDepth::Ansi8);
        let out = colorize_meta("Elapsed time: 0.001849333s", &style);
        let expected = concat!(
            "\x1b[90mElapsed time\x1b[39;49;00m: ",
            "\x1b[34m0.001849333\x1b[39;49;00m\x1b[36ms\x1b[39;49;00m",
            "\x1b[37m\x1b[39;49;00m",
        );
        assert_eq!(out, expected);
    }

    // --- PIE meta (byte-exact) ---

    #[test]
    fn pie_meta_fast_byte_exact() {
        let style = resolve_style("pie", ColorDepth::Ansi256);
        let out = colorize_meta("Elapsed time: 0.002164584s", &style);
        // key grey, colon grey-bold, number green-bold (fast), unit bare.
        // The trailing whitespace token is empty text → 256 emitter drops it.
        let expected = concat!(
            "\x1b[38;5;8mElapsed time\x1b[39m\x1b[38;5;8;01m:\x1b[39;00m ",
            "\x1b[38;5;72;01m0.002164584\x1b[39;00ms",
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn pie_meta_speed_classes() {
        let style = resolve_style("pie", ColorDepth::Ansi256);
        // avg (>0.45, <=1.0) → yellow bold.
        let avg = colorize_meta("Elapsed time: 0.8s", &style);
        assert!(avg.contains("\x1b[38;5;185;01m0.8\x1b[39;00m"), "{avg:?}");
        // slow (>1.0, <=2.5) → orange bold.
        let slow = colorize_meta("Elapsed time: 2.0s", &style);
        assert!(slow.contains("\x1b[38;5;209;01m2.0\x1b[39;00m"), "{slow:?}");
        // very slow (>2.5) → red bold.
        let vslow = colorize_meta("Elapsed time: 3.0s", &style);
        assert!(
            vslow.contains("\x1b[38;5;167;01m3.0\x1b[39;00m"),
            "{vslow:?}"
        );
    }

    // --- Style resolution / downgrade (§3.2, §3.4) ---

    #[test]
    fn resolve_auto_is_generic_regardless_of_depth() {
        assert_eq!(
            resolve_style("auto", ColorDepth::Ansi256),
            resolve_style("auto", ColorDepth::Ansi8)
        );
        assert_eq!(
            resolve_style("auto", ColorDepth::Ansi256).family,
            Family::Ansi8
        );
    }

    #[test]
    fn resolve_pie_on_non_256_downgrades_to_generic() {
        // A pie style on an 8-color terminal is ignored → generic.
        assert_eq!(resolve_style("pie", ColorDepth::Ansi8), generic_style());
    }

    #[test]
    fn resolve_pie_shades_differ() {
        let dark = resolve_style("pie-dark", ColorDepth::Ansi256);
        let mid = resolve_style("pie", ColorDepth::Ansi256);
        let light = resolve_style("pie-light", ColorDepth::Ansi256);
        assert_ne!(dark, mid);
        assert_ne!(mid, light);
        // pie-dark uses shade 500 → header value white (255).
        assert_eq!(dark.header_value, c256(255, false));
        // pie uses shade 600 → header value ansi bright-black (90).
        assert_eq!(mid.header_value, c256_ansi_bright_black(false));
        // pie-light uses shade 700 → header value black (234).
        assert_eq!(light.header_value, c256(234, false));
    }

    #[test]
    fn resolve_named_non_pie_approximates_with_pie_600() {
        // Documented approximation: an arbitrary named style on a 256 term
        // falls back to the pie shade-600 palette.
        assert_eq!(
            resolve_style("monokai", ColorDepth::Ansi256),
            pie_style(Shade::S600)
        );
    }

    // --- Head joining / stripping details ---

    #[test]
    fn head_lines_joined_with_lf_not_crlf() {
        let style = resolve_style("auto", ColorDepth::Ansi8);
        let head = "HTTP/1.1 200 OK\r\nA: b";
        let out = colorize_response_head(head, &style);
        assert!(
            !out.contains('\r'),
            "colorized head must use LF joins: {out:?}"
        );
        assert!(out.contains('\n'));
    }

    #[test]
    fn status_line_empty_reason_stripped() {
        // `HTTP/1.0 200 ` (empty reason): the head is `.strip()`ped, so the
        // trailing space is removed and the code stays visible (§5.4, §11).
        let style = resolve_style("auto", ColorDepth::Ansi8);
        let out = colorize_response_head("HTTP/1.0 200 ", &style);
        assert!(out.ends_with("\x1b[34m200\x1b[39;49;00m"), "{out:?}");
        assert!(!out.ends_with(' '), "{out:?}");
    }

    // --- JSON tokenizer unit checks ---

    #[test]
    fn json_key_vs_value_distinction() {
        let toks = json_tokens(r#"{"k":"v"}"#);
        // Expect: { Punct, "k" Key, : Punct, "v" String, } Punct, trailing WS.
        let kinds: Vec<JsonTok> = toks.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                JsonTok::Punct,
                JsonTok::Key,
                JsonTok::Punct,
                JsonTok::String,
                JsonTok::Punct,
                JsonTok::Whitespace,
            ]
        );
    }

    #[test]
    fn json_number_forms() {
        for n in ["0", "-1", "3.14", "1e10", "-2.5E-3", "42"] {
            let body = format!("[{n}]");
            let toks = json_tokens(&body);
            // Second token (after `[`) is the number, spanning it entirely.
            assert_eq!(toks[1].kind, JsonTok::Number);
            assert_eq!(toks[1].text, n, "number {n}");
        }
    }

    #[test]
    fn json_unterminated_string_does_not_panic() {
        // A body the format stage left as-is may be malformed; lexing must
        // still terminate without panicking.
        let toks = json_tokens(r#"{"a":"unterminated"#);
        assert!(!toks.is_empty());
    }
}
