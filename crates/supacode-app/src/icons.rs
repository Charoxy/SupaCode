//! Icônes de la maquette (traits fins, `currentColor`).

use gpui_kit::component::Icon;
use gpui_kit::*;

use crate::theme::DARK;

macro_rules! svg {
    ($($body:literal),*) => {
        concat!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">"#, $($body,)* "</svg>")
    };
}

pub const CLAUDE: &str = svg!(r#"<path stroke-width="1.8" d="M8 2v12M2 8h12M3.8 3.8l8.4 8.4M12.2 3.8l-8.4 8.4"/>"#);
pub const CODEX: &str = svg!(r#"<path d="M8 1.8l5.4 3.1v6.2L8 14.2l-5.4-3.1V4.9z"/><path d="M8 5.5v5"/>"#);
pub const FOLDER: &str = svg!(r#"<path stroke-width="1.4" d="M2 4.5A1.5 1.5 0 0 1 3.5 3H6l1.5 1.5h5A1.5 1.5 0 0 1 14 6v5.5a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 2 11.5z"/>"#);
pub const CHEVRON_DOWN: &str = svg!(r#"<path d="M4 6l4 4 4-4"/>"#);
pub const CHEVRON_LEFT: &str = svg!(r#"<path d="M10 4L6 8l4 4"/>"#);
pub const EDIT: &str = svg!(r#"<path stroke-width="1.4" d="M10.5 2.5l3 3L6 13H3v-3z"/>"#);
pub const CHEVRON_RIGHT: &str = svg!(r#"<path d="M6 4l4 4-4 4"/>"#);
pub const PLUS: &str = svg!(r#"<path stroke-width="1.6" d="M8 3v10M3 8h10"/>"#);
pub const CLOSE: &str = svg!(r#"<path d="M4.5 4.5l7 7M11.5 4.5l-7 7"/>"#);
pub const STOP: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16" fill="currentColor"><rect x="3" y="3" width="10" height="10" rx="1.5"/></svg>"#;
pub const BRANCH: &str = svg!(r#"<circle stroke-width="1.4" cx="5" cy="3.5" r="1.5"/><circle stroke-width="1.4" cx="5" cy="12.5" r="1.5"/><circle stroke-width="1.4" cx="11" cy="5" r="1.5"/><path stroke-width="1.4" d="M5 5v6M11 6.5c0 3-6 2-6 4.5"/>"#);
pub const CHECK: &str = svg!(r#"<path stroke-width="2" d="M3.5 8.5l3 3 6-7"/>"#);
pub const UNDO: &str = svg!(r#"<path d="M3 6h7a3 3 0 0 1 0 6H6"/><path d="M5.5 3.5L3 6l2.5 2.5"/>"#);
pub const SEARCH: &str = svg!(r#"<circle cx="7" cy="7" r="4.5"/><path d="M10.5 10.5L14 14"/>"#);
pub const BOLT: &str = svg!(r#"<path d="M9 1.5L3.5 9H8l-1 5.5L12.5 7H8z"/>"#);
pub const PLAY: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16" fill="currentColor"><path d="M4.5 2.5v11l9-5.5z"/></svg>"#;
pub const WORKFLOW: &str = svg!(r#"<rect stroke-width="1.4" x="2" y="2" width="5" height="4" rx="1"/><rect stroke-width="1.4" x="9" y="10" width="5" height="4" rx="1"/><path stroke-width="1.4" d="M4.5 6v3a2 2 0 0 0 2 2H9"/>"#);
pub const GLOBE: &str = svg!(r#"<circle stroke-width="1.4" cx="8" cy="8" r="6"/><path stroke-width="1.4" d="M2 8h12M8 2c1.8 2 2.6 4 2.6 6S9.8 12 8 14C6.2 12 5.4 10 5.4 8S6.2 4 8 2z"/>"#);
pub const CURSOR: &str = svg!(r#"<path stroke-width="1.4" d="M3.5 2.5l9 4.2-3.9 1.3-1.3 3.9z"/><path stroke-width="1.4" d="M8.6 8.6l3.6 3.6"/>"#);
pub const MINIMIZE: &str = svg!(r#"<path stroke-width="1.2" d="M3.5 8h9"/>"#);
pub const MAXIMIZE: &str = svg!(r#"<rect stroke-width="1.2" x="3.5" y="3.5" width="9" height="9" rx="1"/>"#);
pub const RESTORE: &str = svg!(r#"<rect stroke-width="1.2" x="3.5" y="5.5" width="7" height="7" rx="1"/><path stroke-width="1.2" d="M5.5 5.5V4.5a1 1 0 0 1 1-1h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-1"/>"#);
pub const SHIELD: &str = svg!(r#"<path stroke-width="1.4" d="M8 1.8l5 1.9v4c0 3.2-2.1 5.6-5 6.5-2.9-.9-5-3.3-5-6.5v-4z"/><path stroke-width="1.4" d="M5.8 8l1.6 1.6 2.9-3"/>"#);
pub const QUOTE: &str = svg!(r#"<path stroke-width="1.4" d="M3 3.5h10a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H8l-3 2.5v-2.5H3a1 1 0 0 1-1-1v-6a1 1 0 0 1 1-1z"/><path stroke-width="1.4" d="M5 6.5h6M5 8.5h4"/>"#);
pub const PUZZLE: &str = svg!(r#"<path stroke-width="1.4" d="M6 2.5a1.5 1.5 0 0 1 3 0V4h3a1 1 0 0 1 1 1v3h-1.5a1.5 1.5 0 0 0 0 3H13v2.5a1 1 0 0 1-1 1H9V13a1.5 1.5 0 0 0-3 0v1.5H3a1 1 0 0 1-1-1V10h1.5a1.5 1.5 0 0 0 0-3H2V5a1 1 0 0 1 1-1h3z"/>"#);
pub const LAYERS: &str = svg!(r#"<path stroke-width="1.4" d="M8 2l6 3-6 3-6-3z"/><path stroke-width="1.4" d="M2 8l6 3 6-3M2 11l6 3 6-3"/>"#);
pub const GRID: &str = svg!(r#"<rect stroke-width="1.4" x="2" y="2" width="5" height="5" rx="1"/><rect stroke-width="1.4" x="9" y="2" width="5" height="5" rx="1"/><rect stroke-width="1.4" x="2" y="9" width="5" height="5" rx="1"/><rect stroke-width="1.4" x="9" y="9" width="5" height="5" rx="1"/>"#);
pub const PANEL_RIGHT: &str = svg!(r#"<rect stroke-width="1.4" x="2" y="3" width="12" height="10" rx="1.5"/><path stroke-width="1.4" d="M10 3v10"/>"#);
pub const PANEL_LEFT: &str = svg!(r#"<rect stroke-width="1.4" x="2" y="3" width="12" height="10" rx="1.5"/><path stroke-width="1.4" d="M6 3v10"/>"#);
pub const AGENTS: &str =svg!(r#"<circle stroke-width="1.4" cx="6" cy="5.5" r="2.2"/><path stroke-width="1.4" d="M2 13c0-2.2 1.8-3.8 4-3.8s4 1.6 4 3.8"/><circle stroke-width="1.4" cx="11.3" cy="6.2" r="1.7"/><path stroke-width="1.4" d="M11 9.3c1.8.1 3 1.5 3 3.4"/>"#);
pub const CUBE: &str = svg!(r#"<path stroke-width="1.4" d="M8 1.8l5.5 3v6.4L8 14.2l-5.5-3V4.8z"/><path stroke-width="1.4" d="M2.7 4.9L8 8l5.3-3.1M8 8v6"/>"#);
pub const SLIDERS: &str = svg!(r#"<path stroke-width="1.4" d="M3 4.5h10M3 11.5h10"/><circle stroke-width="1.4" fill="currentColor" cx="6" cy="4.5" r="1.4"/><circle stroke-width="1.4" fill="currentColor" cx="10.5" cy="11.5" r="1.4"/>"#);
pub const EYE: &str = svg!(r#"<path stroke-width="1.4" d="M1.8 8S4 3.8 8 3.8 14.2 8 14.2 8 12 12.2 8 12.2 1.8 8 1.8 8z"/><circle stroke-width="1.4" cx="8" cy="8" r="2"/>"#);
pub const EYE_OFF: &str = svg!(r#"<path stroke-width="1.4" d="M2.5 2.5l11 11M6.3 4.1A6 6 0 0 1 8 3.8c4 0 6.2 4.2 6.2 4.2a10 10 0 0 1-1.7 2.2M10.4 11.6A5.6 5.6 0 0 1 8 12.2C4 12.2 1.8 8 1.8 8a10 10 0 0 1 2.3-2.8"/>"#);
pub const HISTORY: &str = svg!(r#"<path stroke-width="1.4" d="M2.5 8a5.5 5.5 0 1 0 1.6-3.9"/><path stroke-width="1.4" d="M2.5 2.5v2.5H5M8 5v3l2 1.5"/>"#);
pub const APPROVE: &str = svg!(r#"<circle cx="8" cy="8" r="2.5"/><path d="M1.5 8h4M10.5 8h4"/>"#);
/// Le « S » du logo (posé sur un carré arrondi couleur accent : GPUI dessine les SVG d'une seule couleur).
const LOGO_S: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M13 7.2c-.6-1-1.7-1.5-3-1.5-1.7 0-2.9.9-2.9 2.1 0 2.9 6 1.6 6 4.4 0 1.3-1.3 2.2-3.1 2.2-1.4 0-2.5-.6-3.1-1.6"/></svg>"#;

pub fn icon(svg: &'static str) -> Icon {
    Icon::default().data(svg.as_bytes())
}

/// Logo SupaCode à la taille donnée (20 px dans la barre latérale).
pub fn logo(size: f32) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded(px(size / 4.0))
        .bg(DARK.accent_bg)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(LOGO_S).size(px(size * 0.9)).text_color(DARK.accent_on))
}
