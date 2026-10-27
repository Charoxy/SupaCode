//! Terminal intégré : un vrai PTY (ConPTY sous Windows) piloté par `alacritty_terminal`,
//! rendu cellule par cellule dans GPUI. C'est là que tourne la CLI `claude`.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty::{self, Shell};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui_kit::*;

use supacode_core::process::resolve_program;

use crate::theme::{self, DARK};

/// Hauteur de ligne et taille du texte du terminal (jetons `mono.base` : 13 / 20).
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 20.0;

/// Relaie les événements du terminal (thread du PTY) vers l'interface.
#[derive(Clone)]
struct Listener(async_channel::Sender<TermEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.try_send(event);
    }
}

/// Ce qu'un terminal doit lancer.
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

pub enum TerminalEvent {
    /// Le programme a écrit dans le terminal.
    Output,
    Exited,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

pub struct TerminalView {
    term: Arc<FairMutex<Term<Listener>>>,
    /// Absent si le programme n'a pas pu être lancé (voir `error`).
    sender: Option<EventLoopSender>,
    pub error: Option<String>,
    focus: FocusHandle,
    size: (usize, usize),
    cell_width: Pixels,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub title: Option<String>,
    pub exited: bool,
    _events: Task<()>,
}

impl TerminalView {
    pub fn new(command: Command, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::unbounded();
        let listener = Listener(tx);
        let size = (100, 30);
        let term_size = TermSize::new(size.0, size.1);
        let term = Arc::new(FairMutex::new(Term::new(Config::default(), &term_size, listener.clone())));

        let label = command.program.clone();
        let (sender, error) = match spawn(command, size, term.clone(), listener) {
            Ok(sender) => (Some(sender), None),
            Err(err) => (None, Some(format!("Impossible de lancer « {label} » : {err:#}"))),
        };

        // Événements du PTY → rafraîchissement de la vue (regroupés par lot).
        let events = cx.spawn_in(window, async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                let alive = this.update(cx, |view, cx| {
                    for event in batch {
                        view.handle_event(event, cx);
                    }
                    cx.emit(TerminalEvent::Output);
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        Self {
            term,
            sender,
            error,
            focus: cx.focus_handle(),
            size,
            cell_width: px(8.0),
            bounds: Rc::new(Cell::new(None)),
            title: None,
            exited: false,
            _events: events,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    fn handle_event(&mut self, event: TermEvent, cx: &mut Context<Self>) {
        match event {
            TermEvent::Title(title) => self.title = Some(title),
            TermEvent::ResetTitle => self.title = None,
            TermEvent::PtyWrite(text) => self.write(text.into_bytes()),
            TermEvent::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            TermEvent::ClipboardLoad(_, format) => {
                let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                self.write(format(&text).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                let text = format(window_size(self.size, self.cell_width));
                self.write(text.into_bytes());
            }
            TermEvent::Exit | TermEvent::ChildExit(_) => {
                self.exited = true;
                cx.emit(TerminalEvent::Exited);
            }
            _ => {}
        }
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(Msg::Input(bytes.into()));
        }
    }

    pub fn kill(&self) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(Msg::Shutdown);
        }
    }

    /// Adapte la grille à la place disponible.
    fn resize_to(&mut self, bounds: Bounds<Pixels>) {
        let cols = ((bounds.size.width / self.cell_width).floor() as usize).max(20);
        let rows = ((bounds.size.height / px(LINE_HEIGHT)).floor() as usize).max(5);
        if (cols, rows) == self.size {
            return;
        }
        self.size = (cols, rows);
        self.term.lock().resize(TermSize::new(cols, rows));
        if let Some(sender) = &self.sender {
            let _ = sender.send(Msg::Resize(window_size(self.size, self.cell_width)));
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let mods = keystroke.modifiers;
        // Coller : Ctrl+V (ou Ctrl+Shift+V).
        if mods.control && keystroke.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                self.paste(&text);
            }
            cx.stop_propagation();
            return;
        }
        let app_cursor = self.term.lock().mode().contains(TermMode::APP_CURSOR);
        if let Some(bytes) = key_to_bytes(keystroke, app_cursor) {
            // Toute frappe ramène la vue en bas de l'historique.
            self.term.lock().scroll_display(Scroll::Bottom);
            self.write(bytes);
            cx.stop_propagation();
        }
    }

    fn paste(&self, text: &str) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.term.lock().mode().contains(TermMode::BRACKETED_PASTE) {
            self.write(format!("\x1b[200~{text}\x1b[201~").into_bytes());
        } else {
            self.write(text.into_bytes());
        }
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => delta.y.round() as i32 * 3,
            ScrollDelta::Pixels(delta) => (delta.y / px(LINE_HEIGHT)).round() as i32,
        };
        if lines != 0 {
            self.term.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let font = Font { family: theme::MONO_FONT.into(), ..Font::default() };
        let font_id = window.text_system().resolve_font(&font);
        if let Ok(advance) = window.text_system().advance(font_id, px(FONT_SIZE), 'm') {
            self.cell_width = advance.width;
        }
        if let Some(bounds) = self.bounds.get() {
            self.resize_to(bounds);
        }

        if let Some(error) = &self.error {
            return div().size_full().p_4().text_color(DARK.err).child(error.clone()).into_any_element();
        }
        let (lines, cursor) = self.snapshot(&font);
        let focused = self.focus.is_focused(window);
        let bounds_cell = self.bounds.clone();

        let cursor_el = cursor.filter(|_| focused).map(|(col, line)| {
            div()
                .absolute()
                .left(self.cell_width * col as f32)
                .top(px(LINE_HEIGHT) * line as f32)
                .w(self.cell_width)
                .h(px(LINE_HEIGHT))
                .bg(DARK.text)
                .opacity(0.7)
        });

        div()
            .id("terminal")
            .track_focus(&self.focus)
            .key_context("Terminal")
            .on_key_down(cx.listener(Self::on_key))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)))
            .relative()
            .size_full()
            .overflow_hidden()
            .font_family(theme::MONO_FONT)
            .text_size(px(FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .text_color(DARK.text)
            .child(
                // Mesure la place disponible ; la grille est redimensionnée à l'image suivante.
                canvas(
                    move |bounds, window, _| {
                        if bounds_cell.get() != Some(bounds) {
                            bounds_cell.set(Some(bounds));
                            window.refresh();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(lines.into_iter().map(|(text, runs)| div().h(px(LINE_HEIGHT)).whitespace_nowrap().child(StyledText::new(text).with_runs(runs))))
            .children(cursor_el)
            .into_any_element()
    }
}

impl TerminalView {
    /// Lignes visibles (texte + segments de style) et position du curseur.
    fn snapshot(&self, font: &Font) -> (Vec<(SharedString, Vec<TextRun>)>, Option<(usize, usize)>) {
        let term = self.term.lock();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        let rows = term.screen_lines();
        let mut lines: Vec<(String, Vec<TextRun>)> = (0..rows).map(|_| (String::new(), Vec::new())).collect();

        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset;
            if row < 0 || row as usize >= rows {
                continue;
            }
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let (text, runs) = &mut lines[row as usize];
            let (mut fg, mut bg) = (to_hsla(cell.fg, content.colors), to_hsla(cell.bg, content.colors));
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
                bg = bg.or(Some(theme::hsla(DARK.text)));
            }
            let fg = fg.unwrap_or(theme::hsla(DARK.text));
            let fg = if cell.flags.contains(Flags::DIM) { fg.opacity(0.6) } else { fg };
            let mut cell_font = font.clone();
            if cell.flags.contains(Flags::BOLD) {
                cell_font.weight = FontWeight::SEMIBOLD;
            }
            if cell.flags.contains(Flags::ITALIC) {
                cell_font.style = FontStyle::Italic;
            }
            let ch = if cell.c == '\0' { ' ' } else { cell.c };
            let start = text.len();
            text.push(ch);
            if let Some(zerowidth) = cell.zerowidth() {
                text.extend(zerowidth);
            }
            let len = text.len() - start;
            let underline = cell.flags.intersects(Flags::ALL_UNDERLINES).then(|| UnderlineStyle { thickness: px(1.0), color: Some(fg), wavy: false });
            match runs.last_mut() {
                Some(last) if last.color == fg && last.background_color == bg && last.font == cell_font && last.underline == underline => last.len += len,
                _ => runs.push(TextRun { len, font: cell_font, color: fg, background_color: bg, underline, strikethrough: None }),
            }
        }

        let cursor = content.cursor.point;
        let cursor_row = cursor.line.0 + offset;
        let visible = term.mode().contains(TermMode::SHOW_CURSOR) && cursor_row >= 0 && (cursor_row as usize) < rows;
        let cursor_pos = visible.then_some((cursor.column.0, cursor_row as usize));
        drop(term);

        let lines = lines.into_iter().map(|(text, runs)| (SharedString::from(text), runs)).collect();
        (lines, cursor_pos)
    }
}

/// Lance le programme dans un PTY ; renvoie le canal d'écriture.
fn spawn(command: Command, size: (usize, usize), term: Arc<FairMutex<Term<Listener>>>, listener: Listener) -> anyhow::Result<EventLoopSender> {
    let (program, args) = resolve_program(&command.program, command.args);
    let options = tty::Options {
        shell: Some(Shell::new(program, args)),
        working_directory: Some(command.cwd),
        drain_on_exit: true,
        env: HashMap::from([("TERM".into(), "xterm-256color".into()), ("COLORTERM".into(), "truecolor".into())]),
        #[cfg(windows)]
        escape_args: true,
    };
    let pty = tty::new(&options, window_size(size, px(8.0)), 0)?;
    let event_loop = EventLoop::new(term, listener, pty, false, false)?;
    let sender = event_loop.channel();
    event_loop.spawn();
    Ok(sender)
}

fn window_size((cols, rows): (usize, usize), cell_width: Pixels) -> WindowSize {
    WindowSize { num_lines: rows as u16, num_cols: cols as u16, cell_width: f32::from(cell_width) as u16, cell_height: LINE_HEIGHT as u16 }
}

/// Couleurs ANSI adaptées au thème SupaCode.
fn named(color: NamedColor) -> Option<u32> {
    use NamedColor::*;
    Some(match color {
        Black => 0x1D1F24,
        Red => 0xF26D6A,
        Green => 0x4CC26A,
        Yellow => 0xE0A93E,
        Blue => 0x7DB8FF,
        Magenta => 0xB9A2FF,
        Cyan => 0x33B7A7,
        White => 0xE6E7EA,
        BrightBlack => 0x868B96,
        BrightRed => 0xF47F7C,
        BrightGreen => 0x5BD27E,
        BrightYellow => 0xE7C77D,
        BrightBlue => 0x9496FF,
        BrightMagenta => 0xC9B6FF,
        BrightCyan => 0x5FD4C4,
        BrightWhite => 0xFFFFFF,
        DimBlack => 0x17181C,
        DimRed => 0xB85350,
        DimGreen => 0x3A9452,
        DimYellow => 0xAA8030,
        DimBlue => 0x5F8CC2,
        DimMagenta => 0x8D7BC2,
        DimCyan => 0x278B7F,
        DimWhite => 0xA6AAB3,
        _ => return None,
    })
}

/// `None` = couleur par défaut (premier plan ou fond du thème).
fn to_hsla(color: Color, overrides: &alacritty_terminal::term::color::Colors) -> Option<Hsla> {
    let rgb = match color {
        Color::Spec(rgb) => rgb,
        Color::Indexed(index) => match overrides[index as usize] {
            Some(rgb) => rgb,
            None => indexed(index)?,
        },
        Color::Named(name) => match name {
            NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground | NamedColor::Background => return None,
            _ => match overrides[name as usize] {
                Some(rgb) => rgb,
                None => {
                    let hex = named(name)?;
                    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
                }
            },
        },
    };
    Some(Hsla::from(Rgba { r: rgb.r as f32 / 255.0, g: rgb.g as f32 / 255.0, b: rgb.b as f32 / 255.0, a: 1.0 }))
}

/// Palette 256 couleurs : 16 nommées, cube 6×6×6, puis niveaux de gris.
fn indexed(index: u8) -> Option<Rgb> {
    let rgb = |hex: u32| Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 };
    match index {
        0..16 => {
            let names = [
                NamedColor::Black, NamedColor::Red, NamedColor::Green, NamedColor::Yellow,
                NamedColor::Blue, NamedColor::Magenta, NamedColor::Cyan, NamedColor::White,
                NamedColor::BrightBlack, NamedColor::BrightRed, NamedColor::BrightGreen, NamedColor::BrightYellow,
                NamedColor::BrightBlue, NamedColor::BrightMagenta, NamedColor::BrightCyan, NamedColor::BrightWhite,
            ];
            named(names[index as usize]).map(rgb)
        }
        16..232 => {
            let i = index - 16;
            let step = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
            Some(Rgb { r: step(i / 36), g: step((i / 6) % 6), b: step(i % 6) })
        }
        _ => {
            let v = 8 + (index - 232) * 10;
            Some(Rgb { r: v, g: v, b: v })
        }
    }
}

/// Traduit une touche en séquence pour le terminal.
fn key_to_bytes(keystroke: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let mods = keystroke.modifiers;
    let arrow = |c: char| if app_cursor { format!("\x1bO{c}") } else { format!("\x1b[{c}") };
    let seq: String = match keystroke.key.as_str() {
        "enter" => if mods.alt { "\x1b\r".into() } else { "\r".into() },
        "backspace" => if mods.control { "\x17".into() } else { "\x7f".into() },
        "tab" => if mods.shift { "\x1b[Z".into() } else { "\t".into() },
        "escape" => "\x1b".into(),
        "up" => arrow('A'),
        "down" => arrow('B'),
        "right" => arrow('C'),
        "left" => arrow('D'),
        "home" => "\x1b[H".into(),
        "end" => "\x1b[F".into(),
        "pageup" => "\x1b[5~".into(),
        "pagedown" => "\x1b[6~".into(),
        "delete" => "\x1b[3~".into(),
        "insert" => "\x1b[2~".into(),
        key if mods.control && key.len() == 1 => {
            let c = key.chars().next()?.to_ascii_lowercase();
            match c {
                'a'..='z' => ((c as u8 - b'a' + 1) as char).to_string(),
                '[' => "\x1b".into(),
                '\\' => "\x1c".into(),
                ']' => "\x1d".into(),
                ' ' | '@' => "\0".into(),
                _ => return None,
            }
        }
        _ => {
            let text = keystroke.key_char.clone()?;
            if mods.alt && !mods.control { format!("\x1b{text}") } else { text }
        }
    };
    Some(seq.into_bytes())
}
