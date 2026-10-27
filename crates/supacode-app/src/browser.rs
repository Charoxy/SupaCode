//! Navigateur intégré : une WebView2 (via `wry`) posée sur la fenêtre, à l'emplacement
//! que GPUI lui réserve dans le panneau de droite.
//!
//! GPUI dessine avec DirectComposition au-dessus de ses fenêtres enfants : une WebView
//! enfant resterait invisible. Sous Windows, la page vit donc dans une petite fenêtre
//! sans bordure appartenant à celle de SupaCode (voir `host`), recollée à chaque image.
//!
//! L'utilisateur peut y sélectionner des éléments ou dessiner par-dessus la page, puis
//! envoyer le tout (avec une capture) à l'agent. Les agents le pilotent avec les outils
//! MCP de `crate::mcp`, traités par `call_tool`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{Value, json};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder};

use crate::icons::{self, icon};
use crate::mcp::Content;
use crate::theme::{self, DARK as T};

const INJECT: &str = include_str!("browser/inject.js");

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Browse,
    Select,
    Draw,
}

impl Mode {
    fn js(self) -> &'static str {
        match self {
            Mode::Browse => "browse",
            Mode::Select => "select",
            Mode::Draw => "draw",
        }
    }
}

/// Commentaire prêt à partir vers l'agent : texte et capture de la page annotée.
pub struct Feedback {
    pub text: String,
    pub screenshot: Option<Vec<u8>>,
}

pub enum BrowserEvent {
    Send(Feedback),
}

impl EventEmitter<BrowserEvent> for BrowserView {}

pub struct BrowserView {
    /// Absente le temps que WebView2 démarre (voir `outside_gpui`).
    webview: Option<Rc<WebView>>,
    _context: Option<Rc<RefCell<WebContext>>>,
    error: Option<String>,
    /// Emplacement réservé par GPUI (et position de la fenêtre), appliqué à la vue native.
    placed: Rc<Cell<Option<(Bounds<Pixels>, Point<Pixels>)>>>,
    /// Fenêtre qui porte la page (0 tant qu'elle n'existe pas), et celle de SupaCode.
    host: Rc<Cell<isize>>,
    owner: isize,
    address: Entity<InputState>,
    comment: Entity<InputState>,
    url: String,
    title: String,
    loading: bool,
    mode: Mode,
    picks: Vec<String>,
    strokes: usize,
    sending: bool,
    /// Message affiché au-dessus du champ de commentaire.
    notice: Option<SharedString>,
    shown: bool,
    /// Dernier chargement terminé (pour attendre la page dans `browser_open`).
    loaded_at: Option<Instant>,
    /// Adresse donnée à la création de la vue.
    start: String,
    _tasks: Vec<Task<()>>,
}

impl BrowserView {
    pub fn new(url: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("Adresse ou recherche, par exemple localhost:3000"));
        let comment = cx.new(|cx| InputState::new(window, cx).placeholder("Commentaire pour l'agent…"));
        cx.subscribe_in(&address, window, |this, input, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let text = input.read(cx).value().to_string();
                this.navigate(&text, cx);
            }
        })
        .detach();
        cx.subscribe_in(&comment, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.send(window, cx);
            }
        })
        .detach();

        let (sender, messages) = async_channel::unbounded::<Value>();
        let context = Rc::new(RefCell::new(WebContext::new(dirs::data_local_dir().map(|d| d.join("SupaCode").join("Navigateur")))));
        let start = url.as_deref().map(normalize_url).unwrap_or_else(|| "about:blank".into());
        let (ipc, load, popup) = (sender.clone(), sender.clone(), sender);
        let owner = window.window_handle().map(|h| h.as_raw()).map_err(|e| e.to_string());
        let owner_id = match owner {
            Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
            _ => 0,
        };
        let (ready, built) = async_channel::bounded::<Result<(WebView, isize), String>>(1);
        let (build_context, url) = (context.clone(), start.clone());
        outside_gpui(move || {
            let (parent, host) = match host::create(owner) {
                Ok(created) => created,
                Err(e) => {
                    let _ = ready.try_send(Err(e));
                    return;
                }
            };
            let mut context = build_context.borrow_mut();
            let built = WebViewBuilder::new_with_web_context(&mut context)
                .with_url(&url)
                .with_initialization_script(INJECT)
                .with_devtools(true)
                .with_visible(false)
                .with_bounds(Rect { position: LogicalPosition::new(0.0, 0.0).into(), size: LogicalSize::new(1.0, 1.0).into() })
                .with_ipc_handler(move |request| {
                    if let Ok(message) = serde_json::from_str(request.body()) {
                        let _ = ipc.try_send(message);
                    }
                })
                .with_on_page_load_handler(move |event, url| {
                    let finished = matches!(event, PageLoadEvent::Finished);
                    let _ = load.try_send(json!({ "type": "load", "finished": finished, "url": url }));
                })
                // Les liens « nouvel onglet » s'ouvrent dans la même vue.
                .with_new_window_req_handler(move |url, _| {
                    let _ = popup.try_send(json!({ "type": "open", "url": url }));
                    NewWindowResponse::Deny
                })
                .build_as_child(&parent);
            let _ = ready.try_send(built.map(|webview| (webview, host)).map_err(|e| e.to_string()));
        });

        let arrival = cx.spawn(async move |this, cx| {
            let Ok(built) = built.recv().await else { return };
            let _ = this.update(cx, |this, cx| {
                match built {
                    Ok((webview, host)) => {
                        this.host.set(host);
                        host::show(host, this.shown);
                        let _ = webview.set_visible(true);
                        // La prochaine image place la fenêtre de la page.
                        this.placed.set(None);
                        // Une adresse demandée pendant le démarrage.
                        if this.url != this.start {
                            let _ = webview.load_url(&this.url);
                        }
                        this.webview = Some(Rc::new(webview));
                    }
                    Err(e) => this.error = Some(format!("Impossible d'ouvrir le navigateur (WebView2) : {e}")),
                }
                cx.notify();
            });
        });
        let task = cx.spawn(async move |this, cx| {
            while let Ok(message) = messages.recv().await {
                if this.update(cx, |this, cx| this.on_message(message, cx)).is_err() {
                    break;
                }
            }
        });
        // Fenêtre déplacée ou redimensionnée : la page suit.
        cx.observe_window_bounds(window, |this, _, cx| {
            this.placed.set(None);
            cx.notify();
        })
        .detach();
        address.update(cx, |a, cx| a.set_value(if start == "about:blank" { String::new() } else { start.clone() }, window, cx));
        Self {
            webview: None,
            _context: Some(context),
            error: None,
            placed: Rc::new(Cell::new(None)),
            host: Rc::new(Cell::new(0)),
            owner: owner_id,
            start: start.clone(),
            address,
            comment,
            url: start,
            title: String::new(),
            loading: true,
            mode: Mode::Browse,
            picks: Vec::new(),
            strokes: 0,
            sending: false,
            notice: None,
            shown: false,
            loaded_at: None,
            _tasks: vec![task, arrival],
        }
    }

    /// Montre ou cache la vue native (elle passerait sinon par-dessus le reste de l'interface).
    pub fn set_shown(&mut self, shown: bool) {
        if shown != self.shown {
            self.shown = shown;
            host::show(self.host.get(), shown);
        }
    }

    pub fn notice(&mut self, text: &str, cx: &mut Context<Self>) {
        self.notice = Some(text.to_owned().into());
        cx.notify();
    }

    pub fn title(&self) -> String {
        if self.title.is_empty() { "Navigateur".into() } else { self.title.clone() }
    }

    fn on_message(&mut self, message: Value, cx: &mut Context<Self>) {
        let text = |key: &str| message.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
        match message.get("type").and_then(Value::as_str) {
            Some("load") => {
                let finished = message.get("finished").and_then(Value::as_bool).unwrap_or(false);
                self.loading = !finished;
                if finished {
                    self.loaded_at = Some(Instant::now());
                    // Nouvelle page : les annotations de la précédente sont perdues.
                    self.picks.clear();
                    self.strokes = 0;
                    if self.mode != Mode::Browse {
                        self.eval_quiet(&format!("window.__supacode && __supacode.setMode('{}')", self.mode.js()));
                    }
                }
                self.set_url(text("url"), cx);
            }
            Some("page") => {
                self.title = text("title");
                self.set_url(text("url"), cx);
            }
            Some("open") => {
                let url = text("url");
                self.navigate(&url, cx);
            }
            Some("state") => {
                self.picks = message.get("picks").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                self.strokes = message.get("strokes").and_then(Value::as_u64).unwrap_or(0) as usize;
                if message.get("mode").and_then(Value::as_str) == Some("browse") {
                    self.mode = Mode::Browse;
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn set_url(&mut self, url: String, cx: &mut Context<Self>) {
        if url.is_empty() || url == self.url {
            return;
        }
        self.url = url;
        // L'adresse affichée suit la page, sauf pendant que l'utilisateur la modifie.
        let shown = if self.url == "about:blank" { String::new() } else { self.url.clone() };
        let address = self.address.clone();
        cx.defer(move |cx| {
            // SupaCode n'a qu'une fenêtre, active ou non.
            if let Some(window) = cx.active_window().or_else(|| cx.windows().into_iter().next()) {
                let _ = window.update(cx, |_, window, cx| {
                    address.update(cx, |a, cx| {
                        if !a.focus_handle(cx).is_focused(window) {
                            a.set_value(shown, window, cx);
                        }
                    })
                });
            }
        });
    }

    pub fn navigate(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let url = normalize_url(text);
        if let Some(webview) = &self.webview {
            let _ = webview.load_url(&url);
        }
        self.loading = true;
        self.loaded_at = None;
        self.url = url;
        cx.notify();
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = if self.mode == mode { Mode::Browse } else { mode };
        self.eval_quiet(&format!("__supacode.setMode('{}')", self.mode.js()));
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.eval_quiet("__supacode.clear()");
        self.picks.clear();
        self.strokes = 0;
        cx.notify();
    }

    fn eval_quiet(&self, js: &str) {
        if let Some(webview) = &self.webview {
            let _ = webview.evaluate_script(js);
        }
    }

    /// Exécute `js` (une expression) et renvoie sa valeur, sérialisée par WebView2.
    fn eval_raw(&self, js: &str) -> impl Future<Output = Result<Value>> + use<> {
        let (tx, rx) = async_channel::bounded::<String>(1);
        let wrapped = format!("(function(){{try{{return {{ok:true,value:({js})}}}}catch(e){{return {{ok:false,error:String(e&&e.stack||e)}}}}}})()");
        let started = match &self.webview {
            Some(webview) => webview
                .evaluate_script_with_callback(&wrapped, move |result| {
                    let _ = tx.try_send(result);
                })
                .map_err(|e| anyhow!("{e}")),
            None => Err(anyhow!("le navigateur démarre encore, réessaie dans un instant")),
        };
        async move {
            started?;
            let raw = rx.recv().await.map_err(|_| anyhow!("pas de réponse de la page"))?;
            let value: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
            if value.get("ok").and_then(Value::as_bool) == Some(true) {
                Ok(value.get("value").cloned().unwrap_or(Value::Null))
            } else if let Some(error) = value.get("error").and_then(Value::as_str) {
                bail!("erreur JavaScript : {error}")
            } else {
                bail!("la page n'a pas répondu (chargement en cours ?)")
            }
        }
    }

    /// Appel du protocole DevTools (captures, vrais clics et saisies).
    #[cfg(windows)]
    fn cdp(&self, method: &str, params: Value) -> impl Future<Output = Result<Value>> + use<> {
        use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
        use windows::core::HSTRING;
        use wry::WebViewExtWindows as _;

        let (tx, rx) = async_channel::bounded::<Result<String>>(1);
        let started = (|| -> Result<()> {
            let webview = self.webview.as_ref().ok_or_else(|| anyhow!("le navigateur n'a pas pu s'ouvrir"))?;
            let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
                let _ = tx.try_send(result.map(|_| json).map_err(|e| anyhow!("{e}")));
                Ok(())
            }));
            unsafe { webview.webview().CallDevToolsProtocolMethod(&HSTRING::from(method), &HSTRING::from(params.to_string()), &handler) }?;
            Ok(())
        })();
        async move {
            started?;
            let json = rx.recv().await.map_err(|_| anyhow!("pas de réponse du navigateur"))??;
            Ok(serde_json::from_str(&json).unwrap_or(Value::Null))
        }
    }

    #[cfg(not(windows))]
    fn cdp(&self, _method: &str, _params: Value) -> impl Future<Output = Result<Value>> + use<> {
        async { bail!("disponible seulement sous Windows pour l'instant") }
    }

    fn screenshot(&self) -> impl Future<Output = Result<Vec<u8>>> + use<> {
        use base64::Engine as _;
        let shot = self.cdp("Page.captureScreenshot", json!({ "format": "png" }));
        async move {
            let value = shot.await?;
            let data = value.get("data").and_then(Value::as_str).ok_or_else(|| anyhow!("capture vide"))?;
            Ok(base64::engine::general_purpose::STANDARD.decode(data)?)
        }
    }

    /// Envoie le commentaire, les éléments choisis et une capture de la page annotée.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let comment = self.comment.read(cx).value().trim().to_owned();
        if self.sending || (comment.is_empty() && self.picks.is_empty() && self.strokes == 0) {
            return;
        }
        self.sending = true;
        self.notice = None;
        self.eval_quiet("__supacode.hideHover()");
        let details = self.eval_raw("__supacode.details()");
        let shot = self.screenshot();
        self.comment.update(cx, |c, cx| c.set_value("", window, cx));
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let details = details.await.unwrap_or(Value::Null);
            let screenshot = shot.await.ok();
            let text = feedback_text(&comment, &details);
            let _ = this.update(cx, |this, cx| {
                this.sending = false;
                this.clear(cx);
                if this.mode != Mode::Browse {
                    this.set_mode(this.mode, cx);
                }
                cx.emit(BrowserEvent::Send(Feedback { text, screenshot }));
            });
        });
        self._tasks.push(task);
    }

    fn back(&self) {
        self.eval_quiet("history.back()");
    }

    fn forward(&self) {
        self.eval_quiet("history.forward()");
    }

    fn reload(&self) {
        self.eval_quiet("location.reload()");
    }
}

impl Drop for BrowserView {
    fn drop(&mut self) {
        // La page d'abord, puis la fenêtre qui la porte.
        self.webview = None;
        host::destroy(self.host.get());
    }
}

/// Fenêtre qui porte la page.
#[cfg(windows)]
mod host {
    use std::num::NonZeroIsize;

    use gpui_kit::{Bounds, Pixels, Point};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;
    use wry::WebView;
    use wry::dpi::{LogicalPosition, LogicalSize};
    use wry::raw_window_handle::{RawWindowHandle, Win32WindowHandle};

    use super::ParentWindow;

    unsafe extern "system" fn procedure(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // Un clic dans la page ne doit pas éteindre la barre de titre de SupaCode.
        if message == WM_NCACTIVATE {
            return LRESULT(1);
        }
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    /// Fenêtre sans bordure, hors de la barre des tâches, appartenant à celle de SupaCode
    /// (elle reste donc au-dessus d'elle et se réduit avec elle).
    pub fn create(owner: Result<RawWindowHandle, String>) -> Result<(ParentWindow, isize), String> {
        let owner = match owner? {
            RawWindowHandle::Win32(handle) => handle.hwnd.get(),
            _ => return Err("fenêtre inattendue".into()),
        };
        unsafe {
            let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let class = WNDCLASSW { lpfnWndProc: Some(procedure), hInstance: instance.into(), lpszClassName: w!("SupaCodeNavigateur"), ..Default::default() };
            // Déjà enregistrée si le navigateur a été rouvert : sans importance.
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("SupaCodeNavigateur"),
                w!("Navigateur SupaCode"),
                WS_POPUP | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                Some(HWND(owner as _)),
                None,
                Some(instance.into()),
                None,
            )
            .map_err(|e| e.to_string())?;
            let id = hwnd.0 as isize;
            let handle = Win32WindowHandle::new(NonZeroIsize::new(id).ok_or("fenêtre nulle")?);
            Ok((ParentWindow(RawWindowHandle::Win32(handle)), id))
        }
    }

    /// Colle la fenêtre sur l'emplacement réservé par GPUI (coordonnées de la fenêtre de SupaCode).
    pub fn place(host: isize, _owner: isize, origin: Point<Pixels>, bounds: Bounds<Pixels>, scale: f32, webview: &WebView) {
        if host == 0 {
            return;
        }
        let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        // Les coordonnées de GPUI partent du coin de la fenêtre, barre de titre comprise.
        let point = POINT { x: ((f32::from(origin.x) + x) * scale).round() as i32, y: ((f32::from(origin.y) + y) * scale).round() as i32 };
        unsafe {
            let _ = SetWindowPos(
                HWND(host as _),
                None,
                point.x,
                point.y,
                (width * scale).round() as i32,
                (height * scale).round() as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        let _ = webview.set_bounds(wry::Rect { position: LogicalPosition::new(0.0, 0.0).into(), size: LogicalSize::new(width as f64, height as f64).into() });
    }

    pub fn show(host: isize, shown: bool) {
        if host != 0 {
            unsafe {
                let _ = ShowWindow(HWND(host as _), if shown { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
    }

    pub fn destroy(host: isize) {
        if host != 0 {
            unsafe {
                let _ = DestroyWindow(HWND(host as _));
            }
        }
    }
}

/// Ailleurs : la page est une vue enfant de la fenêtre.
#[cfg(not(windows))]
mod host {
    use gpui_kit::{Bounds, Pixels, Point};
    use wry::WebView;
    use wry::raw_window_handle::RawWindowHandle;

    use super::ParentWindow;

    pub fn create(owner: Result<RawWindowHandle, String>) -> Result<(ParentWindow, isize), String> {
        Ok((ParentWindow(owner?), 0))
    }

    pub fn place(_: isize, _: isize, _: Point<Pixels>, bounds: Bounds<Pixels>, _: f32, webview: &WebView) {
        let _ = webview.set_bounds(super::rect(bounds));
    }

    pub fn show(_: isize, _: bool) {}

    pub fn destroy(_: isize) {}
}

#[cfg_attr(windows, allow(dead_code))]
fn rect(bounds: Bounds<Pixels>) -> Rect {
    Rect {
        position: LogicalPosition::new(f32::from(bounds.origin.x) as f64, f32::from(bounds.origin.y) as f64).into(),
        size: LogicalSize::new(f32::from(bounds.size.width) as f64, f32::from(bounds.size.height) as f64).into(),
    }
}

/// Fenêtre de SupaCode, parente de la vue native.
struct ParentWindow(RawWindowHandle);

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // La fenêtre vit aussi longtemps que l'application.
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

thread_local! {
    static PENDING: RefCell<Vec<Box<dyn FnOnce()>>> = RefCell::new(Vec::new());
}

/// Lance `job` depuis la boucle de messages de Windows, hors de toute mise à jour de GPUI.
/// La création d'une WebView2 fait tourner une boucle de messages imbriquée : depuis une
/// mise à jour, GPUI y serait rappelé alors que l'application est déjà empruntée.
#[cfg(windows)]
fn outside_gpui(job: impl FnOnce() + 'static) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

    unsafe extern "system" fn fire(_: HWND, _: u32, id: usize, _: u32) {
        unsafe {
            let _ = KillTimer(None, id);
        }
        for job in PENDING.with(|p| std::mem::take(&mut *p.borrow_mut())) {
            job();
        }
    }

    PENDING.with(|p| p.borrow_mut().push(Box::new(job)));
    unsafe {
        SetTimer(None, 0, 0, Some(fire));
    }
}

#[cfg(not(windows))]
fn outside_gpui(job: impl FnOnce() + 'static) {
    PENDING.with(|p| p.borrow_mut().push(Box::new(job)));
    for job in PENDING.with(|p| std::mem::take(&mut *p.borrow_mut())) {
        job();
    }
}

/// Texte envoyé à l'agent pour un retour de l'utilisateur sur la page.
fn feedback_text(comment: &str, details: &Value) -> String {
    let url = details.get("url").and_then(Value::as_str).unwrap_or("");
    let title = details.get("title").and_then(Value::as_str).unwrap_or("");
    let mut text = format!("Retour sur la page {url}");
    if !title.is_empty() {
        text.push_str(&format!(" (« {title} »)"));
    }
    text.push_str(" dans le navigateur de SupaCode :\n");
    if !comment.is_empty() {
        text.push_str(&format!("\n{comment}\n"));
    }
    if let Some(picks) = details.get("picks").and_then(Value::as_array).filter(|p| !p.is_empty()) {
        text.push_str("\nÉléments sélectionnés (numérotés sur la capture) :\n");
        for (i, pick) in picks.iter().enumerate() {
            let get = |key: &str| pick.get(key).and_then(Value::as_str).unwrap_or("");
            text.push_str(&format!("{}. <{}> sélecteur `{}`", i + 1, get("tag"), get("selector")));
            if !get("text").is_empty() {
                text.push_str(&format!(", texte « {} »", get("text")));
            }
            if let Some(r) = pick.get("rect") {
                text.push_str(&format!(", position {}×{} à ({}, {})", r["width"], r["height"], r["x"], r["y"]));
            }
            text.push('\n');
            let html = get("html");
            if !html.is_empty() {
                text.push_str(&format!("   HTML : {}\n", html.replace('\n', " ")));
            }
        }
    }
    if details.get("strokes").and_then(Value::as_u64).unwrap_or(0) > 0 {
        text.push_str("\nL'utilisateur a dessiné sur la page (en rouge sur la capture).\n");
    }
    text
}

/// Adresse complète à partir de ce que l'utilisateur tape.
pub fn normalize_url(text: &str) -> String {
    let text = text.trim();
    if text.contains("://") || text.starts_with("about:") || text.starts_with("data:") || text.starts_with("file:") {
        return text.to_owned();
    }
    let host = text.split(['/', '?', '#']).next().unwrap_or("");
    let local = host.starts_with("localhost") || host.starts_with("127.") || host.starts_with("0.0.0.0") || host.starts_with("[::1]");
    if local {
        return format!("http://{text}");
    }
    let looks_like_host = !text.contains(' ') && (host.contains('.') || host.contains(':'));
    if looks_like_host {
        return format!("https://{text}");
    }
    let query: String = text.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else if b == b' ' { "+".into() } else { format!("%{b:02X}") }).collect();
    format!("https://duckduckgo.com/?q={query}")
}

// — Outils des agents

fn target(args: &Value) -> Value {
    json!({ "ref": args.get("ref"), "selector": args.get("selector") })
}

fn check(value: &Value) -> Result<()> {
    match value.get("error").and_then(Value::as_str) {
        Some(error) => bail!("{error}"),
        None => Ok(()),
    }
}

async fn wait_loaded(browser: &WeakEntity<BrowserView>, timeout: Duration, cx: &mut AsyncApp) -> Result<()> {
    let start = Instant::now();
    loop {
        cx.background_executor().timer(Duration::from_millis(150)).await;
        let loaded = browser.read_with(cx, |b, _| b.loaded_at.is_some() && !b.loading)?;
        if loaded || start.elapsed() > timeout {
            return Ok(());
        }
    }
}

async fn eval(browser: &WeakEntity<BrowserView>, js: &str, cx: &mut AsyncApp) -> Result<Value> {
    browser.read_with(cx, |b, _| b.eval_raw(js))?.await
}

async fn cdp(browser: &WeakEntity<BrowserView>, method: &str, params: Value, cx: &mut AsyncApp) -> Result<Value> {
    browser.read_with(cx, |b, _| b.cdp(method, params))?.await
}

async fn press(browser: &WeakEntity<BrowserView>, key: &str, cx: &mut AsyncApp) -> Result<()> {
    let (code, vk, text) = match key {
        "Enter" => ("Enter", 13, "\r"),
        "Tab" => ("Tab", 9, ""),
        "Escape" => ("Escape", 27, ""),
        "Backspace" => ("Backspace", 8, ""),
        "Delete" => ("Delete", 46, ""),
        "ArrowUp" => ("ArrowUp", 38, ""),
        "ArrowDown" => ("ArrowDown", 40, ""),
        "ArrowLeft" => ("ArrowLeft", 37, ""),
        "ArrowRight" => ("ArrowRight", 39, ""),
        "Home" => ("Home", 36, ""),
        "End" => ("End", 35, ""),
        "PageUp" => ("PageUp", 33, ""),
        "PageDown" => ("PageDown", 34, ""),
        "Space" | " " => ("Space", 32, " "),
        other => bail!("touche inconnue : {other}"),
    };
    let key = if key == "Space" { " " } else { key };
    let mut down = json!({ "type": "keyDown", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk });
    if !text.is_empty() {
        down["text"] = json!(text);
    }
    cdp(browser, "Input.dispatchKeyEvent", down, cx).await?;
    cdp(browser, "Input.dispatchKeyEvent", json!({ "type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk }), cx).await?;
    Ok(())
}

fn page_line(value: &Value) -> String {
    format!("Page : {} — {}", value.get("title").and_then(Value::as_str).unwrap_or(""), value.get("url").and_then(Value::as_str).unwrap_or(""))
}

/// Exécute un outil MCP sur le navigateur.
pub async fn call_tool(browser: WeakEntity<BrowserView>, tool: &str, args: Value, cx: &mut AsyncApp) -> Result<Vec<Content>> {
    let text = |s: String| Ok(vec![Content::Text(s)]);
    let arg = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    match tool {
        "browser_open" => {
            let url = arg("url");
            if url.is_empty() {
                bail!("url manquante");
            }
            browser.update(cx, |b, cx| b.navigate(&url, cx))?;
            wait_loaded(&browser, Duration::from_secs(20), cx).await?;
            let page = eval(&browser, "({url: location.href, title: document.title})", cx).await.unwrap_or(Value::Null);
            text(page_line(&page))
        }
        "browser_snapshot" => {
            let value = eval(&browser, "__supacode.snapshot()", cx).await?;
            let mut out = page_line(&value);
            out.push_str("\n\nÉléments interactifs :\n");
            for element in value.get("elements").and_then(Value::as_array).into_iter().flatten() {
                let mut line = format!("[ref={}] <{}>", element["ref"], element["tag"].as_str().unwrap_or(""));
                for key in ["text", "type", "name", "placeholder", "href", "role", "value"] {
                    if let Some(v) = element.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()) {
                        line.push_str(&format!(" {key}=« {v} »"));
                    }
                }
                if let Some(checked) = element.get("checked").and_then(Value::as_bool) {
                    line.push_str(if checked { " coché" } else { " non coché" });
                }
                if element.get("offscreen").is_some() {
                    line.push_str(" (hors écran)");
                }
                out.push_str(&line);
                out.push('\n');
            }
            out.push_str("\nTexte visible :\n");
            out.push_str(value.get("text").and_then(Value::as_str).unwrap_or(""));
            text(out)
        }
        "browser_screenshot" => {
            let png = browser.read_with(cx, |b, _| b.screenshot())?.await?;
            Ok(vec![Content::Image(png)])
        }
        "browser_click" => {
            let at = eval(&browser, &format!("__supacode.locate({})", target(&args)), cx).await?;
            check(&at)?;
            let (x, y) = (at["x"].as_f64().unwrap_or(0.0), at["y"].as_f64().unwrap_or(0.0));
            cdp(&browser, "Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": x, "y": y }), cx).await?;
            cdp(&browser, "Input.dispatchMouseEvent", json!({ "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1 }), cx).await?;
            cdp(&browser, "Input.dispatchMouseEvent", json!({ "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1 }), cx).await?;
            cx.background_executor().timer(Duration::from_millis(300)).await;
            text(format!("Clic sur <{}> « {} ».", at["tag"].as_str().unwrap_or(""), at["text"].as_str().unwrap_or("")))
        }
        "browser_type" => {
            let focused = eval(&browser, &format!("__supacode.focus({})", target(&args)), cx).await?;
            check(&focused)?;
            cdp(&browser, "Input.insertText", json!({ "text": arg("text") }), cx).await?;
            if args.get("submit").and_then(Value::as_bool) == Some(true) {
                press(&browser, "Enter", cx).await?;
                cx.background_executor().timer(Duration::from_millis(300)).await;
            }
            text("Texte saisi.".into())
        }
        "browser_select" => {
            let result = eval(&browser, &format!("__supacode.select({}, {})", target(&args), json!(arg("value"))), cx).await?;
            check(&result)?;
            text(format!("Option choisie : {}", result["value"]))
        }
        "browser_press" => {
            press(&browser, &arg("key"), cx).await?;
            text("Touche envoyée.".into())
        }
        "browser_wait" => {
            let timeout = Duration::from_millis(args.get("timeout_ms").and_then(Value::as_u64).unwrap_or(10_000).min(60_000));
            let js = format!("__supacode.present({}, {})", json!(arg("text")), json!(arg("selector")));
            let start = Instant::now();
            loop {
                if eval(&browser, &js, cx).await.ok().and_then(|v| v.as_bool()) == Some(true) {
                    return text("Trouvé.".into());
                }
                if start.elapsed() > timeout {
                    bail!("rien trouvé après {} ms", timeout.as_millis());
                }
                cx.background_executor().timer(Duration::from_millis(250)).await;
            }
        }
        "browser_eval" => {
            let script = arg("script");
            let js = if script.contains("return") { format!("(function(){{{script}}})()") } else { script };
            let value = eval(&browser, &js, cx).await?;
            text(serde_json::to_string_pretty(&value).unwrap_or_default())
        }
        "browser_console" => {
            let logs = eval(&browser, "__supacode.logs()", cx).await?;
            let errors_only = args.get("errors_only").and_then(Value::as_bool) == Some(true);
            let lines: Vec<String> = logs
                .as_array()
                .into_iter()
                .flatten()
                .filter(|l| !errors_only || l["level"] == "error")
                .map(|l| format!("[{}] {}", l["level"].as_str().unwrap_or(""), l["text"].as_str().unwrap_or("")))
                .collect();
            text(if lines.is_empty() { "Console vide.".into() } else { lines.join("\n") })
        }
        "browser_navigate" => {
            browser.update(cx, |b, cx| {
                b.loaded_at = None;
                b.loading = true;
                match arg("action").as_str() {
                    "back" => b.back(),
                    "forward" => b.forward(),
                    _ => b.reload(),
                }
                cx.notify();
            })?;
            wait_loaded(&browser, Duration::from_secs(10), cx).await?;
            let page = eval(&browser, "({url: location.href, title: document.title})", cx).await.unwrap_or(Value::Null);
            text(page_line(&page))
        }
        other => bail!("outil inconnu : {other}"),
    }
}

// — Rendu

impl Render for BrowserView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = |id: &'static str, svg: &'static str| {
            div()
                .id(id)
                .size(px(26.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .text_color(T.text2)
                .cursor_pointer()
                .hover(|d| d.bg(T.hover).text_color(T.text))
                .child(icon(svg).size(px(14.0)))
        };
        let tool = |id: &'static str, svg: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .h(px(26.0))
                .px_2()
                .flex()
                .items_center()
                .gap_1p5()
                .rounded(px(6.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .border_1()
                .when(active, |d| d.bg(T.accent_bg).border_color(T.accent_bg).text_color(T.accent_on))
                .when(!active, |d| d.border_color(T.border).text_color(T.text2).hover(|d| d.bg(T.hover).text_color(T.text)))
                .child(icon(svg).size(px(13.0)))
                .child(label)
        };
        let toolbar = div()
            .h(px(40.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .border_b_1()
            .border_color(T.border)
            .child(nav("browser-back", icons::CHEVRON_LEFT).on_click(cx.listener(|this, _, _, _| this.back())))
            .child(nav("browser-forward", icons::CHEVRON_RIGHT).on_click(cx.listener(|this, _, _, _| this.forward())))
            .child(nav("browser-reload", icons::UNDO).on_click(cx.listener(|this, _, _, _| this.reload())))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(28.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(T.border)
                    .bg(T.app)
                    .font_family(theme::MONO_FONT)
                    .text_size(px(12.0))
                    .child(icon(icons::GLOBE).size(px(12.0)).text_color(if self.loading { T.accent } else { T.text3 }))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.address).appearance(false))),
            );

        let annotations = div()
            .h(px(40.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .border_b_1()
            .border_color(T.border)
            .child(tool("browser-select", icons::CURSOR, "Sélectionner", self.mode == Mode::Select).on_click(cx.listener(|this, _, _, cx| this.set_mode(Mode::Select, cx))))
            .child(tool("browser-draw", icons::EDIT, "Dessiner", self.mode == Mode::Draw).on_click(cx.listener(|this, _, _, cx| this.set_mode(Mode::Draw, cx))))
            .child(div().flex_1())
            .when(!self.picks.is_empty() || self.strokes > 0, |d| {
                let mut summary = Vec::new();
                if !self.picks.is_empty() {
                    summary.push(format!("{} élément(s)", self.picks.len()));
                }
                if self.strokes > 0 {
                    summary.push(format!("{} trait(s)", self.strokes));
                }
                d.child(div().text_size(px(12.0)).text_color(T.text2).child(summary.join(" · "))).child(
                    div()
                        .id("browser-clear")
                        .h(px(26.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(px(6.0))
                        .text_size(px(12.0))
                        .text_color(T.text3)
                        .cursor_pointer()
                        .hover(|d| d.bg(T.hover).text_color(T.text))
                        .on_click(cx.listener(|this, _, _, cx| this.clear(cx)))
                        .child("Effacer"),
                )
            });

        let picks = (!self.picks.is_empty()).then(|| {
            div().px_2().py_1p5().flex().flex_wrap().gap_1().border_b_1().border_color(T.border).children(self.picks.iter().enumerate().map(|(i, label)| {
                div()
                    .h(px(22.0))
                    .px_1p5()
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded(px(5.0))
                    .bg(T.raised)
                    .text_size(px(11.0))
                    .child(div().text_color(T.accent).font_weight(FontWeight::SEMIBOLD).child((i + 1).to_string()))
                    .child(div().max_w(px(220.0)).truncate().text_color(T.text2).child(label.clone()))
            }))
        });

        let webview = self.webview.clone();
        let (placed, host, owner) = (self.placed.clone(), self.host.clone(), self.owner);
        let page = match &self.error {
            Some(error) => div().flex_1().min_h_0().p_6().flex().items_center().justify_center().text_color(T.err).child(error.clone()).into_any_element(),
            None => div()
                .flex_1()
                .min_h_0()
                .child(
                    canvas(
                        move |bounds, window, _| {
                            let key = (bounds, window.bounds().origin);
                            if placed.get() != Some(key) {
                                if let Some(webview) = &webview {
                                    placed.set(Some(key));
                                    host::place(host.get(), owner, window.bounds().origin, bounds, window.scale_factor(), webview);
                                }
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .size_full(),
                )
                .into_any_element(),
        };

        let can_send = !self.sending;
        let footer = div()
            .flex_shrink_0()
            .p_2()
            .flex()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(T.border)
            .child(div().flex_1().min_w_0().h(px(30.0)).px_2().flex().items_center().rounded(px(6.0)).border_1().border_color(T.border).bg(T.app).child(Input::new(&self.comment).appearance(false)))
            .child(
                div()
                    .id("browser-send")
                    .h(px(30.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded(px(6.0))
                    .bg(T.accent_bg)
                    .text_color(T.accent_on)
                    .font_weight(FontWeight::MEDIUM)
                    .when(can_send, |d| d.cursor_pointer().hover(|d| d.opacity(0.9)))
                    .when(!can_send, |d| d.opacity(0.5))
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
                    .child(icon(icons::CLAUDE).size(px(12.0)))
                    .child(if self.sending { "Envoi…" } else { "Envoyer à l'agent" }),
            );

        let notice = self.notice.clone().map(|text| {
            div().flex_shrink_0().px_3().py_1p5().border_t_1().border_color(T.border).text_size(px(12.0)).text_color(T.warn).child(text)
        });
        div().size_full().flex().flex_col().child(toolbar).child(annotations).children(picks).child(page).children(notice).child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_url;

    #[test]
    fn adresses() {
        assert_eq!(normalize_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize_url("127.0.0.1:8080/a"), "http://127.0.0.1:8080/a");
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("https://x.dev/p?q=1"), "https://x.dev/p?q=1");
        assert_eq!(normalize_url("comment faire"), "https://duckduckgo.com/?q=comment+faire");
    }
}
