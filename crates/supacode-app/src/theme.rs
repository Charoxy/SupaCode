//! Jetons du design SupaCode (maquette Claude Design, planche « Jetons du design »).

use gpui_kit::{Hsla, Rgba};

/// Jeu complet de la maquette : certains jetons ne servent pas encore.
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct Theme {
    pub app: Rgba,
    pub panel: Rgba,
    pub surface: Rgba,
    pub raised: Rgba,
    pub hover: Rgba,
    pub selected: Rgba,
    pub border_subtle: Rgba,
    pub border: Rgba,
    pub border_strong: Rgba,
    pub text: Rgba,
    pub text2: Rgba,
    pub text3: Rgba,
    pub accent: Rgba,
    pub accent_bg: Rgba,
    pub accent_on: Rgba,
    pub claude: Rgba,
    pub claude_soft: Rgba,
    pub codex: Rgba,
    pub codex_soft: Rgba,
    pub ok: Rgba,
    pub ok_soft: Rgba,
    pub err: Rgba,
    pub err_soft: Rgba,
    pub warn: Rgba,
    pub add_bg: Rgba,
    pub add_gutter: Rgba,
    pub add_fg: Rgba,
    pub del_bg: Rgba,
    pub del_gutter: Rgba,
    pub del_fg: Rgba,
}

pub const DARK: Theme = Theme {
    app: c(0x0E0F11),
    panel: c(0x131417),
    surface: c(0x17181C),
    raised: c(0x1D1F24),
    hover: c(0x22242A),
    selected: c(0x23243B),
    border_subtle: c(0x222429),
    border: c(0x2C2F36),
    border_strong: c(0x3A3E47),
    text: c(0xE6E7EA),
    text2: c(0xA6AAB3),
    text3: c(0x868B96),
    accent: c(0x9496FF),
    accent_bg: c(0x5B5BD6),
    accent_on: c(0xFFFFFF),
    claude: c(0xE08A52),
    claude_soft: c(0x352419),
    codex: c(0x33B7A7),
    codex_soft: c(0x11302C),
    ok: c(0x4CC26A),
    ok_soft: c(0x16301E),
    err: c(0xF26D6A),
    err_soft: c(0x3A1B1B),
    warn: c(0xE0A93E),
    add_bg: c(0x132A1B),
    add_gutter: c(0x1B3B25),
    add_fg: c(0x5BD27E),
    del_bg: c(0x2F1718),
    del_gutter: c(0x47201F),
    del_fg: c(0xF47F7C),
};

pub const UI_FONT: &str = "Inter";
pub const MONO_FONT: &str = "JetBrains Mono";

/// Couleur `0xRRGGBB` utilisable dans une constante.
const fn c(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

pub fn hsla(color: Rgba) -> Hsla {
    color.into()
}
