//! SupaCode : l'IDE natif pour coder avec l'IA.

// Pas de console en plus de la fenêtre sous Windows (build optimisé).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod creator;
mod highlight;
mod icons;
mod palette;
mod preview;
mod terminal;
mod theme;
mod workspace;

use std::borrow::Cow;

use gpui_kit::*;
use supacode_core::claude::CLAUDE_SESSION_ENV;

/// Polices de la maquette, embarquées (licence OFL).
const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-SemiBold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
];

fn main() {
    // Si SupaCode est lancé depuis une session Claude Code, ses marqueurs ne doivent pas
    // passer aux conversations (qui se croiraient sous-sessions et ne seraient pas enregistrées).
    for key in CLAUDE_SESSION_ENV {
        // SAFETY : au tout début de `main`, avant le moindre thread.
        unsafe { std::env::remove_var(key) };
    }

    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(|cx| {
        gpui_kit::init(cx);
        if let Err(err) = cx.text_system().add_fonts(FONTS.iter().map(|f| Cow::Borrowed(*f)).collect()) {
            eprintln!("Polices non chargées : {err:#}");
        }
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1440.0), px(900.0)), cx))),
            titlebar: Some(TitlebarOptions { title: Some("SupaCode".into()), ..Default::default() }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| workspace::Workspace::new(window, cx))).expect("ouverture de la fenêtre");
        cx.activate(true);
    });
}
