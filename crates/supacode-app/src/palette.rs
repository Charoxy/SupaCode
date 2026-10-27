//! Palette Ctrl+K : conversations, fichiers, projets et commandes, avec recherche.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use supacode_core::claude::{self, Conversation, Project};
use supacode_core::time::relative;
use supacode_core::workflows::WorkflowFile;

use crate::icons::{self, icon};
use crate::theme::{self, DARK as T};

/// Résultats par groupe dans la vue « Tout ».
const PER_GROUP: usize = 5;
/// Résultats affichés quand un seul type est filtré.
const MAX_RESULTS: usize = 60;
/// Fichiers indexés par projet.
const MAX_FILES: usize = 20_000;
/// Dossiers jamais parcourus (dépendances, sorties de build).
const IGNORED: &[&str] = &[".git", "node_modules", "target", "dist", "out", "build", ".next", ".turbo", "coverage", ".venv", "__pycache__"];

#[derive(Clone)]
pub enum Command {
    NewConversation,
    NewWorkflow,
    AddProject,
    OpenBrowser,
    RunWorkflow(PathBuf, WorkflowFile),
    EditWorkflow(PathBuf, WorkflowFile),
}

#[derive(Clone)]
pub enum Item {
    Conversation(Project, Conversation),
    File { root: PathBuf, relative: String },
    Project(Project),
    Command { label: String, hint: Option<&'static str>, command: Command },
}

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Projects,
    Conversations,
    Files,
    Commands,
}

impl Filter {
    const ORDER: [Filter; 5] = [Filter::All, Filter::Projects, Filter::Conversations, Filter::Files, Filter::Commands];

    fn label(self) -> &'static str {
        match self {
            Filter::All => "Tout",
            Filter::Projects => "Projets",
            Filter::Conversations => "# Conversations",
            Filter::Files => "@ Fichiers",
            Filter::Commands => "> Commandes",
        }
    }

    fn accepts(self, item: &Item) -> bool {
        matches!(
            (self, item),
            (Filter::All, _)
                | (Filter::Projects, Item::Project(_))
                | (Filter::Conversations, Item::Conversation(..))
                | (Filter::Files, Item::File { .. })
                | (Filter::Commands, Item::Command { .. })
        )
    }
}

pub enum PaletteEvent {
    Pick(Item),
    Dismiss,
}

impl EventEmitter<PaletteEvent> for Palette {}

pub struct Palette {
    input: Entity<InputState>,
    filter: Filter,
    /// Conversations de tous les projets (chargées à l'ouverture), puis fichiers, projets, commandes.
    conversations: Vec<Item>,
    files: Vec<Item>,
    projects: Vec<Item>,
    commands: Vec<Item>,
    /// Résultats courants : (groupe, élément, positions des caractères trouvés).
    results: Vec<(&'static str, Item, Vec<usize>)>,
    selected: usize,
    scroll: ScrollHandle,
    _tasks: Vec<Task<()>>,
}

impl Palette {
    pub fn new(projects: Vec<Project>, root: Option<PathBuf>, commands: Vec<Item>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Rechercher une conversation, un fichier, un projet ou une commande…"));
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.selected = 0;
                this.update_results(cx);
            }
        })
        .detach();
        input.update(cx, |input, cx| input.focus(window, cx));

        let mut this = Self {
            input,
            filter: Filter::All,
            conversations: Vec::new(),
            files: Vec::new(),
            projects: projects.iter().cloned().map(Item::Project).collect(),
            commands,
            results: Vec::new(),
            selected: 0,
            scroll: ScrollHandle::new(),
            _tasks: Vec::new(),
        };

        // Conversations de tous les projets et fichiers du projet courant, en arrière-plan.
        let task = cx.spawn(async move |this, cx| {
            let (conversations, files) = cx
                .background_executor()
                .spawn(async move {
                    let dir = claude::projects_dir();
                    let mut conversations: Vec<(Project, Conversation)> = projects
                        .iter()
                        .filter(|p| !p.id.is_empty())
                        .flat_map(|p| dir.as_ref().map(|d| claude::list_conversations(d, &p.id, 30)).unwrap_or_default().into_iter().map(move |c| (p.clone(), c)))
                        .collect();
                    conversations.sort_by_key(|(_, c)| std::cmp::Reverse(c.last_activity));
                    let files = root.map(|r| list_files(&r)).unwrap_or_default();
                    (conversations, files)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.conversations = conversations.into_iter().map(|(p, c)| Item::Conversation(p, c)).collect();
                this.files = files;
                this.update_results(cx);
            });
        });
        this._tasks.push(task);
        this.update_results(cx);
        this
    }

    fn update_results(&mut self, cx: &mut Context<Self>) {
        let raw = self.input.read(cx).value().to_string();
        // « # », « @ » et « > » en tête de recherche filtrent comme les pastilles.
        let (filter, query) = match raw.chars().next() {
            Some('#') => (Filter::Conversations, &raw[1..]),
            Some('@') => (Filter::Files, &raw[1..]),
            Some('>') => (Filter::Commands, &raw[1..]),
            _ => (self.filter, raw.as_str()),
        };
        let query = fold(query.trim());
        let limit = if filter == Filter::All { PER_GROUP } else { MAX_RESULTS };
        let mut results = Vec::new();
        for (group, items) in [("Conversations", &self.conversations), ("Fichiers", &self.files), ("Projets", &self.projects), ("Commandes", &self.commands)] {
            let mut found: Vec<_> = items
                .iter()
                .filter(|item| filter.accepts(item))
                .filter_map(|item| {
                    let text = label(item);
                    let full = fold(&text);
                    // Fichier : une correspondance dans le nom passe avant une correspondance dans le dossier.
                    let name_start = match item {
                        Item::File { .. } => full.iter().rposition(|c| *c == '/').map_or(0, |i| i + 1),
                        _ => 0,
                    };
                    let (score, positions) = match find(&full[name_start..], &query) {
                        Some((score, positions)) => (score, positions.into_iter().map(|p| p + name_start).collect()),
                        None => find(&full, &query).map(|(score, positions)| (score + 100_000, positions))?,
                    };
                    Some((score, item, positions))
                })
                .collect();
            // Sans recherche, l'ordre naturel (le plus récent d'abord) ; sinon le meilleur score.
            if !query.is_empty() {
                found.sort_by_key(|(score, ..)| *score);
            }
            // Sans recherche, pas de liste de fichiers dans « Tout » : elle noierait le reste.
            if query.is_empty() && filter == Filter::All && group == "Fichiers" {
                continue;
            }
            results.extend(found.into_iter().take(limit).map(|(_, item, positions)| (group, item.clone(), positions)));
        }
        self.results = results;
        self.selected = self.selected.min(self.results.len().saturating_sub(1));
        cx.notify();
    }

    /// Remplace la recherche (utilisé pour les tests visuels).
    #[cfg(debug_assertions)]
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let query = query.to_owned();
        self.input.update(cx, |input, cx| input.set_value(query, window, cx));
        self.update_results(cx);
    }

    fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        self.selected = 0;
        self.update_results(cx);
    }

    fn pick(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some((_, item, _)) = self.results.get(index) {
            cx.emit(PaletteEvent::Pick(item.clone()));
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        match key {
            "up" | "down" => {
                let count = self.results.len();
                if count > 0 {
                    self.selected = if key == "up" { (self.selected + count - 1) % count } else { (self.selected + 1) % count };
                    // Les en-têtes de groupe sont aussi des enfants de la liste.
                    let headers = self.results[..=self.selected].windows(2).filter(|w| w[0].0 != w[1].0).count() + 1;
                    self.scroll.scroll_to_item(self.selected + headers);
                }
            }
            "enter" => self.pick(self.selected, cx),
            "escape" => cx.emit(PaletteEvent::Dismiss),
            "tab" => {
                let next = Filter::ORDER.iter().position(|f| *f == self.filter).map_or(0, |i| (i + if event.keystroke.modifiers.shift { 4 } else { 1 }) % 5);
                self.set_filter(Filter::ORDER[next], cx);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for Palette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = SystemTime::now();
        let mut list = div().id("palette-results").max_h(px(480.0)).overflow_y_scroll().track_scroll(&self.scroll).px_2().pt(px(6.0)).pb_2().flex().flex_col();
        let mut previous = None;
        for (index, (group, item, positions)) in self.results.iter().enumerate() {
            if previous != Some(*group) {
                list = list.child(div().px_2().pt(px(if previous.is_none() { 8.0 } else { 10.0 })).pb_1().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(T.text3).child(group.to_uppercase()));
                previous = Some(*group);
            }
            list = list.child(row(index, item, positions, index == self.selected, now, cx));
        }
        if self.results.is_empty() {
            list = list.child(div().px_2().py_6().text_center().text_color(T.text3).child("Aucun résultat"));
        }

        let chips = Filter::ORDER.map(|filter| {
            let on = filter == self.filter;
            div()
                .id(filter.label())
                .h(px(24.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .rounded(px(12.0))
                .border_1()
                .border_color(if on { T.accent_bg } else { T.border })
                .when(on, |d| d.bg(T.selected))
                .text_size(px(12.0))
                .text_color(if on { T.text } else { T.text2 })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.set_filter(filter, cx)))
                .child(filter.label())
        });
        let hint = |keys: &'static str, text: &'static str| div().flex().gap_1().child(div().text_color(T.text2).child(keys)).child(text);

        div()
            .id("palette")
            .w(px(640.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(0x1A1B20))
            .border_1()
            .border_color(T.border_strong)
            .rounded(px(8.0))
            .shadow_lg()
            .capture_key_down(cx.listener(Self::on_key))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Dismiss)))
            .child(
                div()
                    .h(px(48.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px_4()
                    .border_b_1()
                    .border_color(T.border)
                    .child(icon(icons::SEARCH).size(px(16.0)).text_color(T.text3))
                    .child(div().flex_1().text_size(px(15.0)).child(Input::new(&self.input).appearance(false)))
                    .child(div().text_size(px(11.0)).text_color(T.text3).px(px(5.0)).border_1().border_color(T.border).rounded(px(4.0)).child("Échap")),
            )
            .child(div().flex().gap_1().px_3().py_2().border_b_1().border_color(T.border_subtle).children(chips))
            .child(list)
            .child(
                div()
                    .h(px(34.0))
                    .flex()
                    .items_center()
                    .gap_4()
                    .px_4()
                    .border_t_1()
                    .border_color(T.border)
                    .bg(T.surface)
                    .text_size(px(12.0))
                    .text_color(T.text3)
                    .child(hint("↑↓", "naviguer"))
                    .child(hint("↵", "ouvrir"))
                    .child(hint("Tab", "filtrer"))
                    .child(div().flex_1())
                    .child(format!("{} résultat{}", self.results.len(), if self.results.len() > 1 { "s" } else { "" })),
            )
    }
}

fn row(index: usize, item: &Item, positions: &[usize], selected: bool, now: SystemTime, cx: &mut Context<Palette>) -> Stateful<Div> {
    let text = label(item);
    let mono = matches!(item, Item::File { .. });
    let (badge, meta, hint): (AnyElement, Option<String>, Option<&'static str>) = match item {
        Item::Conversation(project, conversation) => (
            div().size(px(18.0)).flex_shrink_0().rounded(px(4.0)).bg(T.claude_soft).flex().items_center().justify_center().child(icon(icons::CLAUDE).size(px(12.0)).text_color(T.claude)).into_any_element(),
            Some(format!("{} · {}", project.name, relative(conversation.last_activity, now))),
            None,
        ),
        Item::File { root, relative } => {
            let ext = Path::new(relative).extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
            let dir = Path::new(relative).parent().map(|p| p.display().to_string().replace('\\', "/")).filter(|p| !p.is_empty());
            let project = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            (
                div().w(px(18.0)).flex_shrink_0().text_center().text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).font_family(theme::MONO_FONT).text_color(rgb(0x7DB8FF)).child(ext.chars().take(3).collect::<String>()).into_any_element(),
                Some(dir.map(|d| format!("{project}/{d}")).unwrap_or(project)),
                None,
            )
        }
        Item::Project(project) => (icon(icons::FOLDER).size(px(16.0)).text_color(T.text2).into_any_element(), Some(project.path.display().to_string()), None),
        Item::Command { hint, command, .. } => (
            icon(match command {
                Command::RunWorkflow(..) => icons::WORKFLOW,
                Command::EditWorkflow(..) => icons::EDIT,
                _ => icons::PLUS,
            }).size(px(16.0)).text_color(T.text2).into_any_element(),
            None,
            *hint,
        ),
    };
    let shown = match item {
        // Pour un fichier, seul le nom est affiché (le dossier va dans la colonne de droite).
        Item::File { relative, .. } => Path::new(relative).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        _ => text.clone(),
    };
    let offset = text.chars().count() - shown.chars().count();
    let positions: Vec<usize> = positions.iter().filter_map(|p| p.checked_sub(offset)).collect();

    div()
        .id(("palette-row", index))
        .h(px(if matches!(item, Item::Conversation(..)) { 36.0 } else { 32.0 }))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(10.0))
        .px_2()
        .rounded(px(6.0))
        .cursor_pointer()
        .when(selected, |d| d.bg(rgb(0x2A2C3F)).border_1().border_color(T.accent_bg))
        .when(!selected, |d| d.hover(|d| d.bg(T.hover)))
        .on_click(cx.listener(move |this, _, _, cx| this.pick(index, cx)))
        .child(badge)
        .child(div().flex_1().min_w_0().truncate().when(mono, |d| d.font_family(theme::MONO_FONT).text_size(px(12.0))).child(highlighted(shown, &positions)))
        .when_some(meta, |d, meta| d.child(div().flex_shrink_0().max_w(px(260.0)).truncate().text_size(px(12.0)).text_color(T.text3).child(meta)))
        .when_some(hint, |d, hint| d.child(div().text_size(px(11.0)).font_family(theme::MONO_FONT).text_color(T.text2).child(hint)))
        .when(selected, |d| d.child(div().text_size(px(11.0)).text_color(T.text2).px(px(5.0)).border_1().border_color(T.border_strong).rounded(px(4.0)).child("↵")))
}

/// Texte avec les caractères trouvés en couleur d'accent.
fn highlighted(text: String, positions: &[usize]) -> StyledText {
    let mut highlights = Vec::new();
    for (i, (start, ch)) in text.char_indices().enumerate() {
        if positions.contains(&i) {
            highlights.push((start..start + ch.len_utf8(), HighlightStyle { color: Some(theme::hsla(T.accent)), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() }));
        }
    }
    StyledText::new(text).with_highlights(highlights)
}

fn label(item: &Item) -> String {
    match item {
        Item::Conversation(_, c) => c.title.clone(),
        Item::File { relative, .. } => relative.clone(),
        Item::Project(p) => p.name.clone(),
        Item::Command { label, .. } => label.clone(),
    }
}

/// Minuscules sans accents, caractère pour caractère (les positions restent valables).
fn fold(text: &str) -> Vec<char> {
    text.chars()
        .map(|c| match c.to_lowercase().next().unwrap_or(c) {
            'à' | 'â' | 'ä' | 'á' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' => 'i',
            'ô' | 'ö' | 'ó' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' => 'c',
            '\\' => '/',
            other => other,
        })
        .collect()
}

/// Score (plus petit = meilleur) et positions trouvées : sous-chaîne d'abord, sinon lettres dans l'ordre.
fn find(text: &[char], query: &[char]) -> Option<(usize, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    if let Some(start) = text.windows(query.len()).position(|w| w == query) {
        // Début du texte, puis début de mot, puis le reste.
        let word_start = start == 0 || !text[start - 1].is_alphanumeric();
        let score = if start == 0 { 0 } else if word_start { 1 } else { 2 };
        return Some((score * 1000 + text.len(), (start..start + query.len()).collect()));
    }
    let mut positions = Vec::with_capacity(query.len());
    let mut chars = text.iter().enumerate();
    for q in query {
        let (i, _) = chars.by_ref().find(|(_, c)| *c == q)?;
        positions.push(i);
    }
    let spread = positions.last().unwrap_or(&0) - positions.first().unwrap_or(&0);
    Some((10_000 + spread * 10 + text.len(), positions))
}

/// Fichiers du projet (chemins relatifs avec « / »), sans les dossiers ignorés.
fn list_files(root: &Path) -> Vec<Item> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if !IGNORED.iter().any(|i| name == *i) {
                    stack.push(path);
                }
            } else if kind.is_file() {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(Item::File { root: root.to_path_buf(), relative: rel.display().to_string().replace('\\', "/") });
                }
                if out.len() >= MAX_FILES {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{find, fold};

    #[test]
    fn recherche_tolerante() {
        let text = fold("Refonte du tunnel de Checkout");
        let (_, positions) = find(&text, &fold("check")).unwrap();
        assert_eq!(positions, (21..26).collect::<Vec<_>>());
        // Accents ignorés, lettres dans l'ordre acceptées.
        assert!(find(&fold("Créer le panier"), &fold("creer")).is_some());
        assert!(find(&fold("src/checkout.test.ts"), &fold("chts")).is_some());
        assert!(find(&fold("panier"), &fold("zz")).is_none());
        // Un début de texte passe avant un milieu de mot.
        let a = find(&fold("checkout.ts"), &fold("check")).unwrap().0;
        let b = find(&fold("recheck.ts"), &fold("check")).unwrap().0;
        assert!(a < b);
    }
}
