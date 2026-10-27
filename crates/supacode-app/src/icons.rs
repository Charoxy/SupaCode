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
