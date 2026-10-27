//! Fenêtre principale : projets et conversations à gauche, la conversation (CLI `claude`)
//! au centre, l'aperçu du code modifié à droite, barre d'état en bas.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use supacode_core::claude::{self, Conversation, Project};
use supacode_core::engine::RunStatus;
use supacode_core::time::relative;
use supacode_core::workflows::{Trigger, WorkflowFile, load_workflows};

use crate::browser::{self, BrowserEvent, BrowserView, Feedback};
use crate::creator::{Creator, CreatorEvent};
use crate::editor::{Editor, EditorEvent};
use crate::highlight::{self, Spans, code_text};
use crate::icons::{self, icon};
use crate::mcp;
use crate::palette::{self, Palette, PaletteEvent};
use crate::preview::{self, ChangedFile, LineKind};
use crate::terminal::{Command, TerminalEvent, TerminalView};
use crate::theme::{self, DARK as T};

mod runs;

use runs::WorkflowRun;

/// Conversations affichées par projet avant « Voir plus ».
const PAGE: usize = 6;
/// Une conversation qui a écrit dans le terminal récemment est « en cours ».
const ACTIVE_WINDOW: Duration = Duration::from_secs(3);
/// En deçà, une conversation est peut-être en cours dans un autre terminal.
const LIVE_SESSION: Duration = Duration::from_secs(60);

struct Tab {
    id: usize,
    project: Project,
    /// Identifiant de session Claude Code (choisi par SupaCode pour une nouvelle conversation).
    conversation_id: String,
    /// Vrai tant que Claude Code n'a pas encore enregistré la conversation.
    unsaved: bool,
    title: String,
    terminal: Entity<TerminalView>,
    /// Fichiers modifiés pendant cette conversation, du plus récent au plus ancien.
    changed: Vec<ChangedFile>,
    selected_file: usize,
    last_output: Option<Instant>,
}

/// Onglet de la barre latérale.
#[derive(Clone, Copy, PartialEq)]
enum SidebarTab {
    Projects,
    Workflows,
}

/// Séparateur glissé entre deux panneaux.
#[derive(Clone, Copy)]
enum Split {
    Sidebar,
    Right,
}

impl Render for Split {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// Largeurs des panneaux, gardées d'une session à l'autre.
#[derive(Clone, Copy)]
struct Layout {
    sidebar: f32,
    right: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self { sidebar: 260.0, right: 576.0 }
    }
}

/// Contenu du panneau de droite.
#[derive(Clone, Copy, PartialEq)]
enum RightPanel {
    /// Code modifié par la conversation active.
    Code,
    /// Suivi d'une exécution de workflow.
    Run(usize),
    /// Navigateur intégré.
    Browser,
    /// Fichier ouvert depuis la palette.
    File,
}

/// Fichier affiché en lecture dans le panneau de droite.
struct OpenedFile {
    relative: String,
    lines: Vec<String>,
    spans: Vec<Spans>,
}

#[derive(Clone, Copy, PartialEq)]
enum PreviewMode {
    File,
    Diff,
}

pub struct Workspace {
    /// Projets ajoutés par l'utilisateur (avec leurs conversations Claude Code).
    projects: Vec<Project>,
    /// Projets connus de Claude Code pas encore ajoutés : proposés à l'ajout.
    suggestions: Vec<Project>,
    /// Liste « Ajouter un projet » ouverte dans la barre latérale.
    adding: bool,
    expanded: HashSet<String>,
    show_all: HashSet<String>,
    conversations: HashMap<String, Vec<Conversation>>,
    selected_project: Option<String>,
    tabs: Vec<Tab>,
    active: Option<usize>,
    next_tab_id: usize,
    workflows: Result<Vec<WorkflowFile>, String>,
    sidebar_tab: SidebarTab,
    /// Barre latérale affichée (un clic sur l'icône de l'onglet ouvert la replie).
    sidebar_open: bool,
    /// Projet choisi dans l'onglet Workflows (sinon : le projet courant).
    workflow_project: Option<String>,
    picking_project: bool,
    layout: Layout,
    layout_dirty: bool,
    runs: Vec<WorkflowRun>,
    next_run_id: usize,
    right: RightPanel,
    preview_mode: PreviewMode,
    confirm_revert: bool,
    branch: Option<String>,
    claude_installed: bool,
    codex_installed: bool,
    pulse: bool,
    window_title: String,
    palette: Option<Entity<Palette>>,
    creator: Option<Entity<Creator>>,
    /// Éditeur visuel : remplace la conversation et le panneau de droite tant qu'il est ouvert.
    editor: Option<Entity<Editor>>,
    opened: Option<OpenedFile>,
    browser: Option<Entity<BrowserView>>,
    /// Fichier `--mcp-config` qui donne aux agents les outils du navigateur.
    mcp_config: Option<PathBuf>,
    /// Focus de la fenêtre quand aucune conversation n'est ouverte (raccourcis clavier).
    focus: FocusHandle,
    watcher: Option<notify::RecommendedWatcher>,
    /// Surveillance du projet courant pour les workflows déclenchés à l'enregistrement.
    save_watcher: Option<(PathBuf, notify::RecommendedWatcher)>,
    save_rules: Vec<(globset::GlobMatcher, String, WorkflowFile)>,
    _tasks: Vec<Task<()>>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            projects: Vec::new(),
            suggestions: Vec::new(),
            adding: false,
            expanded: HashSet::new(),
            show_all: HashSet::new(),
            conversations: HashMap::new(),
            selected_project: None,
            tabs: Vec::new(),
            active: None,
            next_tab_id: 0,
            workflows: Ok(Vec::new()),
            sidebar_tab: SidebarTab::Projects,
            sidebar_open: true,
            workflow_project: None,
            picking_project: false,
            layout: load_layout(),
            layout_dirty: false,
            runs: Vec::new(),
            next_run_id: 0,
            right: RightPanel::Code,
            preview_mode: PreviewMode::File,
            confirm_revert: false,
            branch: None,
            claude_installed: which("claude"),
            codex_installed: which("codex"),
            pulse: true,
            window_title: "SupaCode".into(),
            palette: None,
            creator: None,
            browser: None,
            mcp_config: None,
            editor: None,
            opened: None,
            focus: cx.focus_handle(),
            watcher: None,
            save_watcher: None,
            save_rules: Vec::new(),
            _tasks: Vec::new(),
        };
        this.refresh_projects(cx);
        this.start_mcp(window, cx);
        window.focus(&this.focus, cx);

        // Animation « en cours » (jeton `pulse` : 700 ms) et actualisation de la liste.
        let ticker = cx.spawn_in(window, async move |this, cx| {
            let mut ticks = 0u32;
            loop {
                cx.background_executor().timer(Duration::from_millis(700)).await;
                ticks += 1;
                let alive = this.update(cx, |this, cx| {
                    this.pulse = !this.pulse;
                    if ticks % 43 == 0 {
                        this.refresh_projects(cx);
                    } else if ticks % 4 == 0 {
                        let pending: Vec<Project> = this.tabs.iter().filter(|t| t.unsaved && t.last_output.is_some()).map(|t| t.project.clone()).collect();
                        for project in pending {
                            this.load_conversations(project, cx);
                        }
                    }
                    if this.any_active() || this.current_run().is_some() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        this._tasks.push(ticker);

        // Débogage : `SUPACODE_PALETTE=<recherche>` ouvre la palette au démarrage (captures d'écran de test).
        #[cfg(debug_assertions)]
        if let Ok(query) = std::env::var("SUPACODE_PALETTE") {
            let task = cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.toggle_palette(window, cx);
                    if let Some(palette) = this.palette.clone() {
                        palette.update(cx, |palette, cx| palette.set_query(&query, window, cx));
                    }
                });
            });
            this._tasks.push(task);
        }
        // Débogage : `SUPACODE_PROJECT=<dossier>` sélectionne ce projet ; `SUPACODE_EDIT=<id>` ouvre
        // ensuite l'éditeur sur ce workflow.
        #[cfg(debug_assertions)]
        if let Ok(path) = std::env::var("SUPACODE_PROJECT") {
            let task = cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    let Some(project) = this.projects.iter().find(|p| same_path(&p.path, Path::new(&path))).cloned() else { return };
                    this.selected_project = Some(project_key(&project));
                    this.load_workflows(cx);
                    if let Ok(id) = std::env::var("SUPACODE_EDIT") {
                        let file = load_workflows(&project.path).ok().and_then(|files| files.into_iter().find(|f| f.id == id));
                        if let Some(file) = file {
                            this.open_editor(project.path.clone(), project.name.clone(), Some(file.id), file.workflow, window, cx);
                        }
                    }
                });
            });
            this._tasks.push(task);
        }
        // Débogage : `SUPACODE_CREATE=<description>` ouvre « Nouveau workflow » et lance la génération.
        #[cfg(debug_assertions)]
        if let Ok(description) = std::env::var("SUPACODE_CREATE") {
            let task = cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(2500)).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_creator(window, cx);
                    if let Some(creator) = this.creator.clone() {
                        creator.update(cx, |creator, cx| creator.debug_generate(&description, window, cx));
                    }
                });
            });
            this._tasks.push(task);
        }
        // Débogage : `SUPACODE_OPEN=<chemin relatif>` ouvre ce fichier du projet courant.
        #[cfg(debug_assertions)]
        if let Ok(relative) = std::env::var("SUPACODE_OPEN") {
            let task = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(project) = this.current_project() {
                        this.open_file(&project.path, relative, cx);
                    }
                });
            });
            this._tasks.push(task);
        }
        this
    }

    // — Données

    fn refresh_projects(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            let (projects, suggestions) = cx
                .background_executor()
                .spawn(async move {
                    let known = claude::projects_dir().map(|d| claude::list_projects(&d)).unwrap_or_default();
                    let added = added_projects();
                    let mut projects: Vec<Project> = added
                        .iter()
                        .filter(|path| path.exists())
                        .map(|path| {
                            // Conversations Claude Code du dossier, s'il en a.
                            known.iter().find(|p| same_path(&p.path, path)).cloned().unwrap_or_else(|| Project {
                                id: String::new(),
                                path: path.clone(),
                                name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                                last_activity: SystemTime::UNIX_EPOCH,
                                conversation_count: 0,
                            })
                        })
                        .collect();
                    projects.sort_by_key(|p| std::cmp::Reverse(p.last_activity));
                    let suggestions = known.into_iter().filter(|p| !added.iter().any(|a| same_path(a, &p.path))).collect::<Vec<_>>();
                    (projects, suggestions)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.suggestions = suggestions;
                if this.expanded.is_empty() {
                    if let Some(first) = projects.first() {
                        this.expanded.insert(project_key(first));
                        this.selected_project.get_or_insert_with(|| project_key(first));
                    }
                }
                this.projects = projects;
                let expanded: Vec<Project> = this.projects.iter().filter(|p| this.expanded.contains(&project_key(p))).cloned().collect();
                for project in expanded {
                    this.load_conversations(project, cx);
                }
                this.load_workflows(cx);
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    fn load_conversations(&mut self, project: Project, cx: &mut Context<Self>) {
        if project.id.is_empty() {
            self.conversations.insert(project_key(&project), Vec::new());
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let id = project.id.clone();
            let conversations = cx
                .background_executor()
                .spawn(async move { claude::projects_dir().map(|d| claude::list_conversations(&d, &id, 50)).unwrap_or_default() })
                .await;
            let _ = this.update(cx, |this, cx| {
                for tab in this.tabs.iter_mut().filter(|t| t.unsaved) {
                    if let Some(conv) = conversations.iter().find(|c| c.id == tab.conversation_id) {
                        tab.unsaved = false;
                        tab.title = conv.title.clone();
                    }
                }
                this.conversations.insert(project_key(&project), conversations);
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    /// Workflows du projet courant (onglet actif, sinon projet sélectionné).
    fn load_workflows(&mut self, cx: &mut Context<Self>) {
        self.branch = self.current_project().and_then(|p| preview::git_branch(&p.path));
        let Some(project) = self.workflows_project() else {
            self.workflows = Ok(Vec::new());
            return;
        };
        self.workflows = load_workflows(&project.path).map_err(|e| format!("{e:#}"));
        self.watch_saves(project.path, cx);
        cx.notify();
    }

    /// Déclencheurs `{ "save": "<glob>" }` des workflows du projet courant.
    fn watch_saves(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.save_rules.clear();
        for file in self.workflows.as_ref().map(|w| w.as_slice()).unwrap_or_default() {
            for trigger in file.workflow.triggers() {
                if let Trigger::Save { save } = trigger {
                    if let Ok(glob) = globset::GlobBuilder::new(&save).literal_separator(true).build() {
                        self.save_rules.push((glob.compile_matcher(), save, file.clone()));
                    }
                }
            }
        }
        if self.save_rules.is_empty() {
            self.save_watcher = None;
            return;
        }
        if self.save_watcher.as_ref().is_some_and(|(path, _)| same_path(path, &root)) {
            return;
        }
        let Ok((watcher, rx)) = preview::watch(&root) else {
            self.save_watcher = None;
            return;
        };
        self.save_watcher = Some((root.clone(), watcher));
        let task = cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                cx.background_executor().timer(preview::DEBOUNCE).await;
                let mut paths = vec![first];
                while let Ok(more) = rx.try_recv() {
                    paths.push(more);
                }
                paths.sort();
                paths.dedup();
                let alive = this.update(cx, |this, cx| {
                    // Le projet a changé entre-temps : cette surveillance n'est plus la bonne.
                    if !this.save_watcher.as_ref().is_some_and(|(path, _)| same_path(path, &root)) {
                        return;
                    }
                    for path in &paths {
                        let Ok(relative) = path.strip_prefix(&root) else { continue };
                        let relative = relative.display().to_string().replace('\\', "/");
                        let matching: Vec<_> = this.save_rules.iter().filter(|(glob, ..)| glob.is_match(&relative)).map(|(_, save, file)| (save.clone(), file.clone())).collect();
                        for (save, file) in matching {
                            let context = HashMap::from([("file".to_owned(), relative.clone())]);
                            this.start_workflow(root.clone(), file, format!("save {save}"), context, false, cx);
                        }
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        self._tasks.push(task);
    }

    fn current_project(&self) -> Option<Project> {
        if let Some(tab) = self.active_tab() {
            return Some(tab.project.clone());
        }
        let key = self.selected_project.as_ref()?;
        self.projects.iter().find(|p| &project_key(p) == key).cloned()
    }

    /// Projet dont l'onglet Workflows montre les workflows.
    fn workflows_project(&self) -> Option<Project> {
        self.workflow_project.as_ref().and_then(|key| self.projects.iter().find(|p| &project_key(p) == key).cloned()).or_else(|| self.current_project())
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.tabs.iter().find(|t| t.id == id))
    }

    fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        let id = self.active?;
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    fn any_active(&self) -> bool {
        self.tabs.iter().any(|t| is_active(t))
    }

    // — Actions

    fn toggle_project(&mut self, project: Project, cx: &mut Context<Self>) {
        let key = project_key(&project);
        self.selected_project = Some(key.clone());
        self.workflow_project = None;
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
            self.load_conversations(project, cx);
        }
        if self.active.is_none() {
            self.load_workflows(cx);
        }
        cx.notify();
    }

    /// Ouvre une conversation (ou la ramène au premier plan) ; sans conversation : une nouvelle.
    fn open_conversation(&mut self, project: Project, conversation: Option<Conversation>, prompt: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_conversation_checked(project, conversation, prompt, false, window, cx);
    }

    fn open_conversation_checked(&mut self, project: Project, conversation: Option<Conversation>, prompt: Option<String>, confirmed: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(conv) = &conversation {
            if let Some(tab) = self.tabs.iter().find(|t| t.conversation_id == conv.id) {
                let id = tab.id;
                self.activate(id, window, cx);
                return;
            }
        }
        if let Some(conv) = &conversation {
            let recent = conv.last_activity.elapsed().is_ok_and(|age| age < LIVE_SESSION);
            if recent && !confirmed {
                let answer = window.prompt(
                    PromptLevel::Warning,
                    "Cette conversation est peut-être encore ouverte ailleurs",
                    Some("Elle a été modifiée il y a moins d'une minute. La reprendre ici pendant qu'un autre terminal l'utilise mélangerait les deux sessions."),
                    &["Reprendre ici", "Annuler"],
                    cx,
                );
                let task = cx.spawn_in(window, async move |this, cx| {
                    if answer.await == Ok(0) {
                        let _ = this.update_in(cx, |this, window, cx| this.open_conversation_checked(project, conversation, prompt, true, window, cx));
                    }
                });
                self._tasks.push(task);
                return;
            }
        }
        // Une nouvelle conversation reçoit son identifiant tout de suite : son titre réel
        // remplacera « Nouvelle conversation » dès que Claude Code l'aura enregistrée.
        let session = conversation.as_ref().map(|c| c.id.clone()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut args = Vec::new();
        if let Some(config) = &self.mcp_config {
            // Les deux options prennent plusieurs valeurs : chacune doit être suivie d'une autre option.
            args.extend(["--mcp-config".to_owned(), config.display().to_string(), "--allowedTools".to_owned(), "mcp__supacode".to_owned()]);
        }
        args.extend(match &conversation {
            Some(_) => ["--resume".to_owned(), session.clone()],
            None => ["--session-id".to_owned(), session.clone()],
        });
        args.extend(prompt);
        let command = Command { program: "claude".into(), args, cwd: project.path.clone() };
        let terminal = cx.new(|cx| TerminalView::new(command, window, cx));
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        cx.subscribe(&terminal, move |this, _, event, cx| {
            let Some(tab) = this.tabs.iter_mut().find(|t| t.id == id) else { return };
            match event {
                TerminalEvent::Output => tab.last_output = Some(Instant::now()),
                // La conversation vient d'être enregistrée par Claude Code : on rafraîchit la liste.
                TerminalEvent::Exited => this.refresh_projects(cx),
            }
            cx.notify();
        })
        .detach();
        let title = conversation.as_ref().map(|c| c.title.clone()).unwrap_or_else(|| "Nouvelle conversation".into());
        let unsaved = conversation.is_none();
        self.tabs.push(Tab { id, project: project.clone(), conversation_id: session, unsaved, title, terminal, changed: Vec::new(), selected_file: 0, last_output: None });
        self.selected_project = Some(project_key(&project));
        self.activate(id, window, cx);
    }

    fn activate(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = Some(id);
        self.workflow_project = None;
        self.right = RightPanel::Code;
        self.confirm_revert = false;
        if let Some(tab) = self.active_tab() {
            let focus = tab.terminal.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
            let root = tab.project.path.clone();
            self.watch(root, cx);
        }
        self.load_workflows(cx);
        cx.notify();
    }

    fn close_tab(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.iter().position(|t| t.id == id) {
            let tab = self.tabs.remove(index);
            tab.terminal.read(cx).kill();
            if self.active == Some(id) {
                self.active = None;
                if let Some(next) = self.tabs.get(index.min(self.tabs.len().saturating_sub(1))).map(|t| t.id) {
                    self.activate(next, window, cx);
                } else {
                    self.watcher = None;
                    window.focus(&self.focus, cx);
                }
            }
            cx.notify();
        }
    }

    /// Surveille le projet de la conversation active pour l'aperçu du code.
    fn watch(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let Ok((watcher, rx)) = preview::watch(&root) else {
            self.watcher = None;
            return;
        };
        self.watcher = Some(watcher);
        let task = cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                // Regroupe les écritures successives (un éditeur écrit souvent en plusieurs fois).
                cx.background_executor().timer(preview::DEBOUNCE).await;
                let mut paths = vec![first];
                while let Ok(more) = rx.try_recv() {
                    paths.push(more);
                }
                paths.dedup();
                let root = root.clone();
                let changed: Vec<ChangedFile> = cx
                    .background_executor()
                    .spawn(async move { paths.iter().filter_map(|p| preview::diff_file(&root, p)).filter(|f| f.added + f.removed > 0).collect() })
                    .await;
                let alive = this.update(cx, |this, cx| {
                    if let Some(tab) = this.active_tab_mut() {
                        for file in changed {
                            tab.changed.retain(|f| f.path != file.path);
                            tab.changed.insert(0, file);
                            tab.selected_file = 0;
                        }
                    }
                    this.confirm_revert = false;
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        self._tasks.push(task);
    }

    /// Ajoute un dossier à la liste (sans rien créer ni modifier dedans).
    fn add_project_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        save_added_project(&path);
        let key = path.display().to_string().to_lowercase();
        self.expanded.insert(key.clone());
        self.selected_project = Some(key);
        self.adding = false;
        self.refresh_projects(cx);
    }

    /// Retire le projet de la liste ; le dossier et ses conversations restent intacts.
    fn remove_project(&mut self, project: &Project, cx: &mut Context<Self>) {
        remove_added_project(&project.path);
        let key = project_key(project);
        self.expanded.remove(&key);
        if self.selected_project.as_ref() == Some(&key) {
            self.selected_project = None;
        }
        self.projects.retain(|p| project_key(p) != key);
        self.refresh_projects(cx);
        cx.notify();
    }

    fn add_project(&mut self, cx: &mut Context<Self>) {
        self.adding = false;
        let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Ajouter le projet".into()) });
        let task = cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| this.add_project_path(path, cx));
        });
        self._tasks.push(task);
    }

    fn new_conversation(&mut self, prompt: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(project) = self.current_project() {
            self.open_conversation(project, None, prompt, window, cx);
        } else {
            self.add_project(cx);
        }
    }

    fn revert_selected(&mut self, cx: &mut Context<Self>) {
        if !self.confirm_revert {
            self.confirm_revert = true;
            cx.notify();
            return;
        }
        self.confirm_revert = false;
        if let Some(tab) = self.active_tab_mut() {
            if let Some(file) = tab.changed.get(tab.selected_file).cloned() {
                if preview::revert(&tab.project.path, &file).is_ok() {
                    tab.changed.remove(tab.selected_file);
                    tab.selected_file = 0;
                }
            }
        }
        cx.notify();
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.refocus(window, cx);
            cx.notify();
            return;
        }
        let mut commands = vec![
            palette::Item::Command { label: "Nouvelle conversation".into(), hint: None, command: palette::Command::NewConversation },
            palette::Item::Command { label: "Nouveau workflow…".into(), hint: None, command: palette::Command::NewWorkflow },
            palette::Item::Command { label: "Ajouter un projet".into(), hint: None, command: palette::Command::AddProject },
            palette::Item::Command { label: "Ouvrir le navigateur".into(), hint: None, command: palette::Command::OpenBrowser },
        ];
        if let (Some(project), Ok(workflows)) = (self.workflows_project(), &self.workflows) {
            for file in workflows {
                commands.push(palette::Item::Command {
                    label: format!("Workflow : lancer « {} »", file.workflow.name),
                    hint: None,
                    command: palette::Command::RunWorkflow(project.path.clone(), file.clone()),
                });
                commands.push(palette::Item::Command {
                    label: format!("Workflow : modifier « {} »", file.workflow.name),
                    hint: None,
                    command: palette::Command::EditWorkflow(project.path.clone(), file.clone()),
                });
            }
        }
        let root = self.current_project().map(|p| p.path);
        let projects = self.projects.clone();
        let palette = cx.new(|cx| Palette::new(projects, root, commands, window, cx));
        cx.subscribe_in(&palette, window, |this, _, event: &PaletteEvent, window, cx| {
            this.palette = None;
            if let PaletteEvent::Pick(item) = event {
                this.pick(item.clone(), window, cx);
            } else {
                this.refocus(window, cx);
            }
            cx.notify();
        })
        .detach();
        self.palette = Some(palette);
        cx.notify();
    }

    fn pick(&mut self, item: palette::Item, window: &mut Window, cx: &mut Context<Self>) {
        match item {
            palette::Item::Conversation(project, conversation) => {
                let key = project_key(&project);
                if self.expanded.insert(key) {
                    self.load_conversations(project.clone(), cx);
                }
                self.open_conversation(project, Some(conversation), None, window, cx);
            }
            palette::Item::File { root, relative } => {
                self.open_file(&root, relative, cx);
                self.refocus(window, cx);
            }
            palette::Item::Project(project) => {
                let key = project_key(&project);
                self.selected_project = Some(key.clone());
                if self.expanded.insert(key) {
                    self.load_conversations(project, cx);
                }
                if self.active.is_none() {
                    self.load_workflows(cx);
                }
                self.refocus(window, cx);
            }
            palette::Item::Command { command, .. } => match command {
                palette::Command::NewConversation => self.new_conversation(None, window, cx),
                palette::Command::NewWorkflow => self.open_creator(window, cx),
                palette::Command::AddProject => self.add_project(cx),
                palette::Command::OpenBrowser => {
                    self.ensure_browser(window, cx);
                }
                palette::Command::EditWorkflow(path, file) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.open_editor(path, name, Some(file.id), file.workflow, window, cx);
                }
                palette::Command::RunWorkflow(path, file) => {
                    self.run_workflow(path, file, cx);
                    self.refocus(window, cx);
                }
            },
        }
    }

    /// Fenêtre « Nouveau workflow » pour le projet courant.
    fn open_creator(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.workflows_project() else {
            self.add_project(cx);
            return;
        };
        let creator = cx.new(|cx| Creator::new(project.path.clone(), project.name.clone(), window, cx));
        cx.subscribe_in(&creator, window, |this, _, event: &CreatorEvent, window, cx| {
            this.creator = None;
            if let CreatorEvent::Edit(workflow) = event {
                if let Some(project) = this.workflows_project() {
                    this.open_editor(project.path, project.name, None, workflow.clone(), window, cx);
                }
                return;
            }
            if let CreatorEvent::Created(id) = event {
                this.sidebar_tab = SidebarTab::Workflows;
                this.load_workflows(cx);
                if let Some(project) = this.workflows_project() {
                    this.open_file(&project.path, format!(".supacode/workflows/{id}.json"), cx);
                }
            }
            this.refocus(window, cx);
            cx.notify();
        })
        .detach();
        self.creator = Some(creator);
        cx.notify();
    }

    fn open_editor(&mut self, project: PathBuf, project_name: String, id: Option<String>, workflow: supacode_core::workflows::Workflow, window: &mut Window, cx: &mut Context<Self>) {
        let editor = cx.new(|cx| Editor::new(project.clone(), project_name, id, workflow, window, cx));
        cx.subscribe_in(&editor, window, move |this, _, event: &EditorEvent, window, cx| {
            match event {
                EditorEvent::Close => {
                    this.editor = None;
                    this.refocus(window, cx);
                }
                EditorEvent::Saved { id, run } => {
                    this.load_workflows(cx);
                    if *run {
                        // « Tester » : on quitte l'éditeur pour suivre l'exécution.
                        this.editor = None;
                        let file = load_workflows(&project).ok().and_then(|files| files.into_iter().find(|f| &f.id == id));
                        if let Some(file) = file {
                            this.run_workflow(project.clone(), file, cx);
                        }
                    }
                }
            }
            cx.notify();
        })
        .detach();
        self.editor = Some(editor);
        cx.notify();
    }

    /// Rend le clavier à la conversation active.
    /// Serveur MCP du navigateur : les appels des agents sont exécutés ici, dans l'interface.
    fn start_mcp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let server = match mcp::start() {
            Ok(server) => server,
            Err(e) => {
                eprintln!("serveur MCP indisponible : {e:#}");
                return;
            }
        };
        self.mcp_config = mcp::write_claude_config(&server.url);
        let calls = server.calls;
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Ok(call) = calls.recv().await {
                let Ok(browser) = this.update_in(cx, |this, window, cx| this.ensure_browser(window, cx).downgrade()) else { break };
                cx.spawn(async move |cx| {
                    let result = browser::call_tool(browser, &call.tool, call.args, cx).await;
                    let _ = call.reply.send(result).await;
                })
                .detach();
            }
        });
        self._tasks.push(task);
    }

    /// Ouvre le navigateur dans le panneau de droite (en le créant au besoin).
    fn ensure_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<BrowserView> {
        self.right = RightPanel::Browser;
        cx.notify();
        if let Some(browser) = &self.browser {
            return browser.clone();
        }
        let browser = cx.new(|cx| BrowserView::new(None, window, cx));
        cx.subscribe_in(&browser, window, |this, _, event: &BrowserEvent, window, cx| match event {
            BrowserEvent::Send(feedback) => this.send_feedback(feedback, window, cx),
        })
        .detach();
        self.browser = Some(browser.clone());
        browser
    }

    fn toggle_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.browser.is_some() && self.right == RightPanel::Browser {
            self.right = RightPanel::Code;
            cx.notify();
        } else {
            self.ensure_browser(window, cx);
        }
    }

    fn close_browser(&mut self, cx: &mut Context<Self>) {
        if let Some(browser) = self.browser.take() {
            browser.update(cx, |b, _| b.set_shown(false));
        }
        if self.right == RightPanel::Browser {
            self.right = RightPanel::Code;
        }
        cx.notify();
    }

    /// Retour de l'utilisateur sur une page : enregistre la capture et l'envoie à la conversation ouverte.
    fn send_feedback(&mut self, feedback: &Feedback, window: &mut Window, cx: &mut Context<Self>) {
        let mut text = feedback.text.clone();
        if self.active_tab().is_none() && self.current_project().is_none() {
            if let Some(browser) = &self.browser {
                browser.update(cx, |b, cx| b.notice("Ajoute un projet ou ouvre une conversation pour envoyer ce retour à un agent.", cx));
            }
            return;
        }
        if let Some(png) = &feedback.screenshot {
            let dir = match self.current_project() {
                Some(project) => project.path.join(".supacode").join("captures"),
                None => std::env::temp_dir().join("SupaCode").join("captures"),
            };
            let name = format!("capture-{}.png", SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
            let saved = std::fs::create_dir_all(&dir).and_then(|_| {
                // Les captures ne doivent pas finir dans le dépôt.
                let ignore = dir.join(".gitignore");
                if !ignore.exists() {
                    std::fs::write(ignore, "*\n")?;
                }
                std::fs::write(dir.join(&name), png)
            });
            if saved.is_ok() {
                text.push_str(&format!("\nCapture d'écran annotée : {}\n", dir.join(&name).display()));
            }
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("SUPACODE_FEEDBACK_LOG").is_some() {
            eprintln!("— retour envoyé —\n{text}");
            return;
        }
        match self.active_tab() {
            Some(tab) if !tab.terminal.read(cx).exited => {
                let terminal = tab.terminal.clone();
                terminal.read(cx).paste(text.trim_end());
                // Entrée un peu après le collage, pour que Claude Code l'ait bien reçu.
                let task = cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(Duration::from_millis(150)).await;
                    let _ = terminal.update(cx, |t, _| t.write(b"\r".as_slice()));
                });
                self._tasks.push(task);
            }
            _ => self.new_conversation(Some(text), window, cx),
        }
    }

    fn refocus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = match self.active_tab() {
            Some(tab) => tab.terminal.read(cx).focus_handle().clone(),
            None => self.focus.clone(),
        };
        window.focus(&focus, cx);
    }

    fn open_file(&mut self, root: &Path, relative: String, cx: &mut Context<Self>) {
        let path = root.join(&relative);
        let lines = match std::fs::metadata(&path) {
            Ok(meta) if meta.len() > 2_000_000 => vec!["(fichier trop volumineux pour l'aperçu)".to_owned()],
            _ => match std::fs::read(&path) {
                Ok(bytes) if bytes.contains(&0) => vec!["(fichier binaire)".to_owned()],
                Ok(bytes) => String::from_utf8_lossy(&bytes).lines().map(highlight::expand_tabs).collect(),
                Err(err) => vec![format!("(lecture impossible : {err})")],
            },
        };
        let spans = highlight::highlight(&relative, &lines);
        self.opened = Some(OpenedFile { relative, lines, spans });
        self.right = RightPanel::File;
        cx.notify();
    }

    fn accept_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab_mut() {
            if tab.selected_file < tab.changed.len() {
                tab.changed.remove(tab.selected_file);
                tab.selected_file = 0;
            }
        }
        cx.notify();
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.active_tab().map(|t| format!("{} — {}", t.project.name, t.title)).unwrap_or_else(|| "SupaCode".into());
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
        let overlay = self.palette.clone().map(|palette| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(rgba(0x0607099e))
                .flex()
                .justify_center()
                .items_start()
                .pt(px(96.0))
                .child(palette)
        });
        let creator = self.creator.clone().map(|creator| {
            div().absolute().top_0().left_0().size_full().bg(rgba(0x0607099e)).flex().justify_center().items_start().pt(px(84.0)).child(creator)
        });
        let activity = self.render_activity_bar(cx);
        let browser_shown = self.right == RightPanel::Browser && self.palette.is_none() && self.creator.is_none() && self.editor.is_none();
        if let Some(browser) = &self.browser {
            browser.update(cx, |b, _| b.set_shown(browser_shown));
        }
        let sidebar = self.sidebar_open.then(|| self.render_sidebar(window, cx).into_any_element());
        let (center, right) = if self.editor.is_none() {
            (Some(self.render_center(window, cx).into_any_element()), self.render_right(cx).map(|r| r.into_any_element()))
        } else {
            (None, None)
        };
        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            // En phase de capture : passe avant le terminal, qui enverrait sinon Ctrl+K à `claude`.
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let k = &event.keystroke;
                if k.modifiers.control && !k.modifiers.alt && (k.key == "k" || k.key == "p") {
                    this.toggle_palette(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<Split>, _, cx| {
                let x = f32::from(event.event.position.x - event.bounds.origin.x);
                let width = f32::from(event.bounds.size.width);
                match *event.drag(cx) {
                    Split::Sidebar => this.layout.sidebar = (x - ACTIVITY_BAR).clamp(MIN_SIDEBAR, (width - ACTIVITY_BAR - this.layout.right - MIN_CENTER).clamp(MIN_SIDEBAR, MAX_SIDEBAR)),
                    Split::Right => {
                        let sidebar = if this.sidebar_open { this.layout.sidebar } else { 0.0 };
                        this.layout.right = (width - x).clamp(MIN_RIGHT, (width - ACTIVITY_BAR - sidebar - MIN_CENTER).max(MIN_RIGHT))
                    }
                }
                this.layout_dirty = true;
                cx.notify();
            }))
            // Un clic dans SupaCode reprend le clavier à la page web.
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                if let Some(browser) = &this.browser {
                    browser.read(cx).release_focus();
                }
            }))
            .capture_any_mouse_up(cx.listener(|this, _, _, _| {
                if std::mem::take(&mut this.layout_dirty) {
                    save_layout(this.layout);
                }
            }))
            .flex()
            .flex_col()
            .bg(T.app)
            .text_color(T.text)
            .font_family(theme::UI_FONT)
            .text_size(px(13.0))
            .line_height(px(18.0))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(activity)
                    .when_some(sidebar, |d, sidebar| d.child(sidebar).child(splitter("split-sidebar", Split::Sidebar, T.panel)))
                    .when_some(self.editor.clone(), |d, editor| d.child(div().flex_1().min_w_0().child(editor)))
                    .children(center)
                    .when_some(right, |d, right| d.child(splitter("split-right", Split::Right, T.surface)).child(right)),
            )
            .child(self.render_status(cx))
            .children(overlay)
            .children(creator)
    }
}

// — Rendu

impl Workspace {
    fn render_sidebar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().id("projects").flex_1().min_h_0().overflow_y_scroll().px_2().flex().flex_col().gap(px(2.0));
        if self.projects.is_empty() {
            list = list.child(
                div()
                    .m_1()
                    .p_4()
                    .border_1()
                    .border_dashed()
                    .border_color(T.border_strong)
                    .rounded(px(6.0))
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Aucun projet"))
                    .child(div().text_size(px(12.0)).text_color(T.text2).child("Ajoute tes projets avec « Ajouter un projet » en bas : un dossier de ta machine, ou un projet sur lequel tu as déjà travaillé avec Claude Code.")),
            );
        }
        let active_conversation = self.active_tab().map(|t| t.conversation_id.clone());
        for (index, project) in self.projects.clone().into_iter().enumerate() {
            let key = project_key(&project);
            let open = self.expanded.contains(&key);
            let busy = self.tabs.iter().any(|t| same_path(&t.project.path, &project.path) && is_active(t));
            let selected = self.selected_project.as_ref() == Some(&key);
            let toggle = project.clone();
            let removed = project.clone();
            let group = SharedString::from(format!("project-row-{index}"));
            list = list.child(
                div()
                    .id(("project", index))
                    .group(group.clone())
                    .h(px(28.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_1p5()
                    .rounded(px(6.0))
                    .when(selected, |d| d.bg(T.hover))
                    .hover(|d| d.bg(T.hover))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_project(toggle.clone(), cx)))
                    .child(icon(if open { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT }).size(px(14.0)).text_color(T.text3))
                    .child(icon(icons::FOLDER).size(px(14.0)).text_color(T.text2))
                    .child(div().flex_1().min_w_0().truncate().font_weight(if open { FontWeight::MEDIUM } else { FontWeight::NORMAL }).child(project.name.clone()))
                    .when(busy, |d| d.child(div().size(px(6.0)).rounded_full().bg(T.claude).opacity(if self.pulse { 1.0 } else { 0.35 })))
                    .when(project.conversation_count > 0, |d| {
                        d.child(
                            div()
                                .text_size(px(11.0))
                                .min_w(px(18.0))
                                .px(px(5.0))
                                .rounded(px(9.0))
                                .bg(T.hover)
                                .text_color(T.text2)
                                .flex()
                                .justify_center()
                                .child(project.conversation_count.to_string()),
                        )
                    })
                    .child(
                        div()
                            .id(("project-remove", index))
                            .size(px(18.0))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .text_color(T.text3)
                            .opacity(0.0)
                            .group_hover(group, |d| d.opacity(1.0))
                            .hover(|d| d.bg(T.raised).text_color(T.text))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.remove_project(&removed, cx);
                            }))
                            .child(icon(icons::CLOSE).size(px(11.0))),
                    ),
            );
            if !open {
                continue;
            }

            let conversations = self.conversations.get(&key).cloned().unwrap_or_default();
            let all = self.show_all.contains(&key);
            let mut items = div().flex().flex_col().flex_shrink_0().gap(px(2.0)).pl(px(14.0));
            // Nouvelles conversations ouvertes dans ce projet, pas encore enregistrées par Claude Code.
            let unsaved: Vec<_> = self
                .tabs
                .iter()
                .filter(|t| t.unsaved && project_key(&t.project) == key && !conversations.iter().any(|c| c.id == t.conversation_id))
                .map(|t| (t.id, t.title.clone(), Some(t.id) == self.active, is_active(t)))
                .collect();
            for (tab_id, title, selected, busy) in &unsaved {
                let tab_id = *tab_id;
                items = items.child(conversation_row(("new", tab_id), title.clone().into(), None, *selected, self.pulse, *busy).on_click(cx.listener(move |this, _, window, cx| this.activate(tab_id, window, cx))));
            }
            if conversations.is_empty() && unsaved.is_empty() {
                items = items.child(div().h(px(30.0)).px_2().flex().items_center().text_size(px(12.0)).text_color(T.text3).child("Aucune conversation"));
            }
            let now = SystemTime::now();
            for (i, conversation) in conversations.iter().enumerate().take(if all { usize::MAX } else { PAGE }) {
                let is_selected = active_conversation.as_deref() == Some(conversation.id.as_str());
                let busy = self.tabs.iter().any(|t| t.conversation_id == conversation.id && is_active(t));
                let date = (!busy).then(|| relative(conversation.last_activity, now));
                let (project, conversation) = (project.clone(), conversation.clone());
                items = items.child(
                    conversation_row(("conv", index * 1000 + i), conversation.title.clone().into(), date, is_selected, self.pulse, busy)
                        .on_click(cx.listener(move |this, _, window, cx| this.open_conversation(project.clone(), Some(conversation.clone()), None, window, cx))),
                );
            }
            if !all && conversations.len() > PAGE {
                let more_key = key.clone();
                items = items.child(
                    div()
                        .id(("more", index))
                        .h(px(26.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .text_size(px(12.0))
                        .text_color(T.accent)
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show_all.insert(more_key.clone());
                            cx.notify();
                        }))
                        .child(format!("Voir {} de plus", conversations.len() - PAGE)),
                );
            }
            list = list.child(items);
        }

        let projects_tab = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(list.pt_1())
            .child(
                div().border_t_1().border_color(T.border).p_2().when(self.adding, |d| d.child(self.render_add_menu(cx))).child(
                    div()
                        .id("add-project")
                        .h(px(30.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(6.0))
                        .text_color(T.text2)
                        .cursor_pointer()
                        .hover(|d| d.bg(T.hover))
                        .when(self.adding, |d| d.bg(T.hover))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.adding = !this.adding;
                            cx.notify();
                        }))
                        .child(icon(icons::PLUS).size(px(14.0)))
                        .child("Ajouter un projet"),
                ),
            );
        let body = match self.sidebar_tab {
            SidebarTab::Projects => projects_tab.into_any_element(),
            SidebarTab::Workflows => self.render_workflows(cx).into_any_element(),
        };
        let (title, count) = match self.sidebar_tab {
            SidebarTab::Projects => ("Projets", self.projects.len()),
            SidebarTab::Workflows => ("Workflows", self.workflows.as_ref().map(|w| w.len()).unwrap_or(0)),
        };

        div()
            .w(px(self.layout.sidebar))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(T.panel)
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(section_label(title).flex_1())
                            .when(count > 0, |d| d.child(div().text_size(px(11.0)).text_color(T.text3).child(count.to_string()))),
                    )
                    .child(
                        div()
                            .id("new-conversation")
                            .h(px(30.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_1p5()
                            .rounded(px(6.0))
                            .bg(T.accent_bg)
                            .text_color(T.accent_on)
                            .font_weight(FontWeight::MEDIUM)
                            .cursor_pointer()
                            .hover(|d| d.opacity(0.9))
                            .on_click(cx.listener(|this, _, window, cx| this.new_conversation(None, window, cx)))
                            .child(icon(icons::PLUS).size(px(14.0)))
                            .child("Nouvelle conversation"),
                    ),
            )
            .child(body)
    }

    /// Barre d'icônes tout à gauche : choisit l'onglet de la barre latérale.
    fn render_activity_bar(&self, cx: &mut Context<Self>) -> Div {
        let running = self.runs.iter().any(|r| r.running());
        div()
            .w(px(ACTIVITY_BAR))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .pt(px(10.0))
            .bg(T.app)
            .border_r_1()
            .border_color(T.border)
            .child(div().mb_2().child(icons::logo(24.0)))
            .child(self.activity_button("activity-projects", icons::FOLDER, SidebarTab::Projects, false, cx))
            .child(self.activity_button("activity-workflows", icons::WORKFLOW, SidebarTab::Workflows, running, cx))
            .child(div().w(px(24.0)).h(px(1.0)).my_1().bg(T.border))
            .child({
                let selected = self.browser.is_some() && self.right == RightPanel::Browser;
                div()
                    .id("activity-browser")
                    .relative()
                    .size(px(36.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .text_color(if selected { T.text } else { T.text3 })
                    .when(selected, |d| d.bg(T.hover))
                    .hover(|d| d.text_color(T.text))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_browser(window, cx)))
                    .child(icon(icons::GLOBE).size(px(18.0)))
            })
    }

    fn activity_button(&self, id: &'static str, svg: &'static str, tab: SidebarTab, badge: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let selected = self.sidebar_open && self.sidebar_tab == tab;
        div()
            .id(id)
            .relative()
            .size(px(36.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.0))
            .cursor_pointer()
            .text_color(if selected { T.text } else { T.text3 })
            .when(selected, |d| d.bg(T.hover))
            .hover(|d| d.text_color(T.text))
            .on_click(cx.listener(move |this, _, _, cx| {
                // L'onglet déjà ouvert : replie la barre latérale.
                this.sidebar_open = !(this.sidebar_open && this.sidebar_tab == tab);
                this.sidebar_tab = tab;
                this.adding = false;
                this.picking_project = false;
                cx.notify();
            }))
            .when(selected, |d| d.child(div().absolute().left(px(-6.0)).top(px(8.0)).w(px(2.0)).h(px(20.0)).rounded(px(1.0)).bg(T.accent)))
            .child(icon(svg).size(px(18.0)))
            .when(badge, |d| d.child(div().absolute().top(px(7.0)).right(px(7.0)).size(px(7.0)).rounded_full().bg(T.accent).opacity(if self.pulse { 1.0 } else { 0.4 })))
    }

    /// Choix du projet à ajouter : un dossier, ou un projet déjà utilisé avec Claude Code.
    fn render_add_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let now = SystemTime::now();
        let rows = self.suggestions.iter().take(30).enumerate().map(|(i, project)| {
            let path = project.path.clone();
            div()
                .id(("suggestion-project", i))
                .h(px(30.0))
                .flex_shrink_0()
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(|d| d.bg(T.hover))
                .on_click(cx.listener(move |this, _, _, cx| this.add_project_path(path.clone(), cx)))
                .child(icon(icons::FOLDER).size(px(13.0)).text_color(T.text3))
                .child(div().flex_1().min_w_0().truncate().child(project.name.clone()))
                .child(div().flex_shrink_0().text_size(px(11.0)).text_color(T.text3).child(relative(project.last_activity, now)))
        });
        div()
            .mb_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .id("add-folder")
                    .h(px(30.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(6.0))
                    .bg(T.raised)
                    .border_1()
                    .border_color(T.border)
                    .cursor_pointer()
                    .hover(|d| d.bg(T.hover))
                    .on_click(cx.listener(|this, _, _, cx| this.add_project(cx)))
                    .child(icon(icons::FOLDER).size(px(13.0)).text_color(T.accent))
                    .child("Ouvrir un dossier…"),
            )
            .when(!self.suggestions.is_empty(), |d| {
                d.child(section_label("Récents dans Claude Code").px_2().pt_2())
                    .child(div().id("suggestion-list").max_h(px(220.0)).overflow_y_scroll().flex().flex_col().gap(px(2.0)).children(rows))
            })
    }

    fn render_workflows(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().id("workflow-list").flex_1().min_h_0().overflow_y_scroll().px_2().pt_1().flex().flex_col().gap(px(2.0));
        let current = self.workflows_project();
        match &self.workflows {
            Err(error) => body = body.child(div().px_1p5().py_1().text_size(px(12.0)).text_color(T.err).child(format!("✖ {error}"))),
            Ok(_) if current.is_none() => {
                body = body.child(div().px_1p5().py_1().text_size(px(12.0)).text_color(T.text3).child("Ajoute un projet pour voir ses workflows."));
            }
            Ok(list) if list.is_empty() => {
                body = body.child(
                    div()
                        .px_1p5()
                        .py_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_size(px(12.0))
                        .text_color(T.text3)
                        .child("Aucun workflow dans ce projet.")
                        .child(
                            div()
                                .id("create-workflow")
                                .text_color(T.accent)
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| this.open_creator(window, cx)))
                                .child("Décrire un workflow…"),
                        ),
                );
            }
            Ok(list) => {
                let project = current.as_ref().map(|p| p.path.clone());
                for (i, file) in list.iter().enumerate() {
                    let tags = file.workflow.triggers().iter().map(|t| tag(t.label())).collect::<Vec<_>>();
                    let run = project.as_ref().and_then(|p| self.run_for(p, &file.id));
                    let shown = run.is_some_and(|r| self.right == RightPanel::Run(r.id));
                    let running = run.filter(|r| r.running());
                    let run_id = run.map(|r| r.id);
                    let button = div()
                        .id(("wf-action", i))
                        .size(px(24.0))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(T.border)
                        .rounded(px(6.0))
                        .bg(T.panel)
                        .text_color(T.text2)
                        .cursor_pointer()
                        .hover(|d| d.bg(T.raised).text_color(T.text));
                    let button = match (running.map(|r| r.id), &project) {
                        (Some(id), _) => button
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.stop_run(id, cx);
                            }))
                            .child(icon(icons::STOP).size(px(10.0))),
                        (None, Some(path)) => {
                            let (path, file) = (path.clone(), file.clone());
                            button
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.run_workflow(path.clone(), file.clone(), cx);
                                }))
                                .child(icon(icons::PLAY).size(px(10.0)))
                        }
                        (None, None) => button,
                    };
                    let finished = run.filter(|r| !r.running()).and_then(|r| r.status);
                    let edit_button = project.clone().map(|path| {
                        let file = file.clone();
                        div()
                            .id(("wf-edit", i))
                            .size(px(24.0))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .text_color(T.text3)
                            .cursor_pointer()
                            .hover(|d| d.bg(T.raised).text_color(T.text))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                this.open_editor(path.clone(), name, Some(file.id.clone()), file.workflow.clone(), window, cx);
                            }))
                            .child(icon(icons::EDIT).size(px(12.0)))
                    });
                    body = body.child(
                        div()
                            .id(("workflow", i))
                            .px_1p5()
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(shown, |d| d.bg(T.hover))
                            .when_some(run_id, |d, id| {
                                d.cursor_pointer().hover(|d| d.bg(T.hover)).on_click(cx.listener(move |this, _, _, cx| {
                                    this.right = RightPanel::Run(id);
                                    cx.notify();
                                }))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(div().truncate().child(file.workflow.name.clone()))
                                    .child(div().flex().flex_wrap().gap_1().children(tags)),
                            )
                            .when_some(running, |d, run| {
                                let (done, total) = run.progress();
                                d.child(div().text_size(px(11.0)).text_color(T.accent).child(format!("{}/{total}", (done + 1).min(total))))
                            })
                            .when_some(finished, |d, status| {
                                let (mark, color) = match status {
                                    RunStatus::Success => (icons::CHECK, T.ok),
                                    RunStatus::Failed => (icons::CLOSE, T.err),
                                    RunStatus::Cancelled => (icons::STOP, T.text3),
                                };
                                d.child(icon(mark).size(px(12.0)).text_color(color))
                            })
                            .children(edit_button)
                            .child(button),
                    );
                }
            }
        }
        let picker = (self.picking_project && self.projects.len() > 1).then(|| {
            let current = current.as_ref().map(project_key);
            div().mx_2().mb_1().p_1().rounded(px(6.0)).border_1().border_color(T.border).bg(T.app).flex().flex_col().gap(px(2.0)).children(
                self.projects.iter().enumerate().map(|(i, project)| {
                    let key = project_key(project);
                    let selected = current.as_ref() == Some(&key);
                    div()
                        .id(("wf-project-choice", i))
                        .h(px(28.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .hover(|d| d.bg(T.hover))
                        .when(selected, |d| d.bg(T.hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.workflow_project = Some(key.clone());
                            this.picking_project = false;
                            this.load_workflows(cx);
                        }))
                        .child(icon(icons::FOLDER).size(px(13.0)).text_color(T.text3))
                        .child(div().flex_1().min_w_0().truncate().child(project.name.clone()))
                        .when(selected, |d| d.child(icon(icons::CHECK).size(px(12.0)).text_color(T.accent)))
                }),
            )
        });
        let can_pick = self.projects.len() > 1;
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .when_some(current.clone(), |d, project| {
                d.child(
                    div()
                        .id("wf-project")
                        .mx_2()
                        .mt_1()
                        .mb_1()
                        .h(px(30.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(6.0))
                        .text_color(T.text2)
                        .when(can_pick, |d| {
                            d.cursor_pointer().hover(|d| d.bg(T.hover)).on_click(cx.listener(|this, _, _, cx| {
                                this.picking_project = !this.picking_project;
                                cx.notify();
                            }))
                        })
                        .child(icon(icons::FOLDER).size(px(13.0)).text_color(T.text3))
                        .child(div().flex_1().min_w_0().truncate().text_color(T.text).child(project.name))
                        .when(can_pick, |d| d.child(icon(if self.picking_project { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT }).size(px(12.0)).text_color(T.text3))),
                )
            })
            .children(picker)
            .child(body)
            .child(
                div().border_t_1().border_color(T.border).p_2().child(
                    div()
                        .id("new-workflow")
                        .h(px(30.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(6.0))
                        .text_color(T.text2)
                        .cursor_pointer()
                        .hover(|d| d.bg(T.hover))
                        .on_click(cx.listener(|this, _, window, cx| this.open_creator(window, cx)))
                        .child(icon(icons::PLUS).size(px(14.0)))
                        .child("Nouveau workflow"),
                ),
            )
    }

    fn render_center(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let center = div().flex_1().min_w_0().flex().flex_col().bg(T.surface);
        let Some(active) = self.active_tab() else {
            return center.child(self.render_welcome(cx));
        };
        let (terminal, cwd, active_id, exited) = (active.terminal.clone(), active.project.path.display().to_string(), active.id, active.terminal.read(cx).exited);

        let mut tabs = div().flex().items_stretch().min_w_0().flex_1().overflow_hidden();
        for tab in &self.tabs {
            let id = tab.id;
            let is_active_tab = Some(id) == self.active;
            tabs = tabs.child(
                div()
                    .id(("tab", id))
                    .w(px(196.0))
                    .flex_shrink(1.0)
                    .min_w(px(100.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px(px(10.0))
                    .border_r_1()
                    .border_color(T.border)
                    .cursor_pointer()
                    .when(is_active_tab, |d| d.bg(T.surface).border_t_2().border_color(T.claude))
                    .on_click(cx.listener(move |this, _, window, cx| this.activate(id, window, cx)))
                    .child(div().size(px(6.0)).flex_shrink_0().rounded_full().bg(T.claude).opacity(if is_active(tab) && !self.pulse { 0.35 } else { 1.0 }))
                    .child(div().flex_1().min_w_0().truncate().text_color(if is_active_tab { T.text } else { T.text3 }).child(tab.title.clone()))
                    .child(
                        div()
                            .id(("close", id))
                            .rounded(px(4.0))
                            .hover(|d| d.bg(T.hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(id, window, cx);
                            }))
                            .child(icon(icons::CLOSE).size(px(12.0)).text_color(T.text3)),
                    ),
            );
        }
        tabs = tabs.child(
            div()
                .id("new-tab")
                .w(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|d| d.bg(T.hover))
                .on_click(cx.listener(|this, _, window, cx| this.new_conversation(None, window, cx)))
                .child(icon(icons::PLUS).size(px(14.0)).text_color(T.text3)),
        );

        let header = div()
            .h(px(36.0))
            .flex_shrink_0()
            .flex()
            .items_stretch()
            .border_b_1()
            .border_color(T.border)
            .bg(T.panel)
            .child(tabs)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .flex_shrink_0()
                    .child(
                        div()
                            .h(px(22.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .rounded(px(4.0))
                            .bg(T.claude_soft)
                            .text_color(T.claude)
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(icon(icons::CLAUDE).size(px(11.0)))
                            .child("Claude"),
                    )
                    .child(div().max_w(px(260.0)).truncate().text_size(px(12.0)).text_color(T.text3).font_family(theme::MONO_FONT).child(cwd))
                    .child(if exited {
                        div().text_size(px(12.0)).text_color(T.text3).child("terminée").into_any_element()
                    } else {
                        div()
                            .id("stop")
                            .h(px(24.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .border_1()
                            .border_color(T.border)
                            .rounded(px(6.0))
                            .text_color(T.err)
                            .text_size(px(12.0))
                            .cursor_pointer()
                            .hover(|d| d.bg(T.hover))
                            .on_click(cx.listener(move |this, _, window, cx| this.close_tab(active_id, window, cx)))
                            .child(icon(icons::STOP).size(px(10.0)))
                            .child("Arrêter")
                            .into_any_element()
                    }),
            );

        center.child(header).child(div().flex_1().min_h_0().pt(px(14.0)).px_4().child(terminal))
    }

    fn render_welcome(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self.current_project();
        let agent = |name: &'static str, command: &'static str, svg: &'static str, color: Rgba, soft: Rgba, installed: bool| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .px(px(14.0))
                .py_3()
                .child(div().size(px(24.0)).rounded(px(6.0)).bg(soft).flex().items_center().justify_center().child(icon(svg).size(px(14.0)).text_color(color)))
                .child(div().font_weight(FontWeight::MEDIUM).child(name))
                .child(div().text_size(px(12.0)).text_color(T.text3).font_family(theme::MONO_FONT).child(command))
                .child(div().flex_1())
                .child(if installed {
                    div().flex().items_center().gap_1p5().text_size(px(12.0)).text_color(T.ok).child(icon(icons::CHECK).size(px(12.0))).child("Installé")
                } else {
                    div().text_size(px(12.0)).text_color(T.warn).child("Non installé")
                })
        };
        let suggestions = [
            "Explique-moi l'architecture de ce projet",
            "Trouve et corrige les tests qui échouent",
            "Ajoute des tests pour le module le moins couvert",
            "Relis les derniers changements et signale les problèmes",
        ];

        div().size_full().flex().justify_center().overflow_hidden().child(
            div()
                .w_full()
                .max_w(px(720.0))
                .px_8()
                .pt(px(96.0))
                .flex()
                .flex_col()
                .gap_8()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(icons::logo(40.0))
                        .child(div().mt_2().text_size(px(28.0)).line_height(px(34.0)).font_weight(FontWeight::SEMIBOLD).child("Bienvenue dans SupaCode"))
                        .child(div().text_size(px(15.0)).line_height(px(22.0)).text_color(T.text2).child(match &project {
                            Some(p) => format!("Projet sélectionné : {}. Ouvre une conversation à gauche, ou démarre-en une. Le code modifié s'affiche à droite.", p.name),
                            None => "Ouvre un projet, puis confie une tâche à Claude. Le code modifié s'affiche à côté de la conversation.".into(),
                        })),
                )
                .child(
                    div().flex().gap_3().child(
                        div()
                            .id("welcome-add")
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap_3()
                            .p_4()
                            .border_1()
                            .border_color(T.accent_bg)
                            .rounded(px(6.0))
                            .bg(T.raised)
                            .cursor_pointer()
                            .hover(|d| d.bg(T.hover))
                            .on_click(cx.listener(|this, _, _, cx| this.add_project(cx)))
                            .child(div().size(px(32.0)).rounded(px(6.0)).bg(T.selected).flex().items_center().justify_center().child(icon(icons::FOLDER).size(px(16.0)).text_color(T.accent)))
                            .child(div().flex().flex_col().gap(px(2.0)).child(div().font_weight(FontWeight::MEDIUM).child("Ouvrir un dossier")).child(div().text_size(px(12.0)).text_color(T.text2).child("Un projet déjà sur ta machine"))),
                    ),
                )
                .when(self.projects.is_empty() && !self.suggestions.is_empty(), |d| {
                    let cards = self.suggestions.iter().take(6).enumerate().map(|(i, project)| {
                        let path = project.path.clone();
                        div()
                            .id(("welcome-suggestion", i))
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .p_3()
                            .border_1()
                            .border_color(T.border)
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .hover(|d| d.bg(T.hover))
                            .on_click(cx.listener(move |this, _, _, cx| this.add_project_path(path.clone(), cx)))
                            .child(icon(icons::FOLDER).size(px(14.0)).text_color(T.text2))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().truncate().font_weight(FontWeight::MEDIUM).child(project.name.clone()))
                                    .child(div().truncate().text_size(px(12.0)).text_color(T.text3).child(format!("{} conversation(s)", project.conversation_count))),
                            )
                            .child(icon(icons::PLUS).size(px(12.0)).text_color(T.text3))
                    });
                    d.child(div().flex().flex_col().gap(px(10.0)).child(section_label("Ajouter un projet récent de Claude Code")).child(div().grid().grid_cols(3).gap_2().children(cards)))
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(section_label("Agents"))
                        .child(
                            div()
                                .border_1()
                                .border_color(T.border)
                                .rounded(px(6.0))
                                .overflow_hidden()
                                .child(agent("Claude Code", "claude", icons::CLAUDE, T.claude, T.claude_soft, self.claude_installed).border_b_1().border_color(T.border))
                                .child(agent("Codex", "codex", icons::CODEX, T.codex, T.codex_soft, self.codex_installed)),
                        ),
                )
                .when_some(project, |d, _| {
                    d.child(
                        div().flex().flex_col().gap(px(10.0)).child(section_label("Pour démarrer")).child(div().grid().grid_cols(2).gap_2().children(suggestions.into_iter().enumerate().map(|(i, text)| {
                            div()
                                .id(("suggestion", i))
                                .flex()
                                .gap(px(10.0))
                                .p_3()
                                .border_1()
                                .border_color(T.border)
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .hover(|d| d.bg(T.hover))
                                .on_click(cx.listener(move |this, _, window, cx| this.new_conversation(Some(text.to_owned()), window, cx)))
                                .min_w_0()
                                .child(div().text_color(T.text3).font_family(theme::MONO_FONT).child(">"))
                                .child(div().flex_1().min_w_0().child(text))
                        }))),
                    )
                }),
        )
    }

    fn render_right(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let panel = div().w(px(self.layout.right)).min_w(px(self.layout.right)).max_w(px(self.layout.right)).overflow_hidden().flex_shrink_0().flex().flex_col().bg(T.surface).min_h_0();
        if self.right == RightPanel::Browser {
            if let Some(browser) = self.browser.clone() {
                return Some(panel.child(self.render_right_tabs(None, cx)).child(div().flex_1().min_h_0().child(browser)));
            }
        }
        if let RightPanel::Run(id) = self.right {
            if let Some(view) = self.render_run(id, cx) {
                return Some(panel.child(self.render_right_tabs(None, cx)).child(view));
            }
        }
        if self.right == RightPanel::File {
            if let Some(opened) = &self.opened {
                let lines = opened.lines.iter().enumerate().map(|(i, line)| {
                    div()
                        .flex()
                        .h(px(20.0))
                        .child(div().w(px(48.0)).flex_shrink_0().pr_3().flex().justify_end().text_color(T.text3).child((i + 1).to_string()))
                        .child(div().whitespace_nowrap().child(code_text(line, opened.spans.get(i).unwrap_or(&Vec::new()))))
                });
                let toolbar = div()
                    .h(px(36.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .border_b_1()
                    .border_color(T.border)
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.0)).text_color(T.text2).font_family(theme::MONO_FONT).child(opened.relative.replace('/', " › ")))
                    .child(div().text_size(px(12.0)).text_color(T.text3).child(format!("{} lignes", opened.lines.len())));
                return Some(
                    panel
                        .child(self.render_right_tabs(None, cx))
                        .child(toolbar)
                        .child(div().id("opened-file").flex_1().min_h_0().overflow_scroll().py_2().font_family(theme::MONO_FONT).text_size(px(13.0)).line_height(px(20.0)).children(lines)),
                );
            }
        }
        let tab = self.active_tab()?;
        let changed = tab.changed.clone();
        let selected = tab.selected_file.min(changed.len().saturating_sub(1));

        let Some(file) = changed.get(selected).cloned() else {
            return Some(
                panel.when(!self.runs.is_empty() || self.opened.is_some(), |d| d.child(self.render_right_tabs(None, cx))).child(
                    div().size_full().flex().flex_col().items_center().justify_center().gap_2().p_8().child(div().font_weight(FontWeight::MEDIUM).child("Aperçu du code")).child(
                        div()
                            .text_color(T.text3)
                            .text_size(px(12.0))
                            .text_center()
                            .child("Les fichiers que Claude crée ou modifie dans ce projet s'ouvriront ici, avec leurs changements."),
                    ),
                ),
            );
        };

        let file_tabs = self.render_right_tabs(Some(selected), cx);

        let mode_button = |label: &'static str, mode: PreviewMode, current: PreviewMode| {
            div()
                .id(label)
                .h(px(22.0))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(4.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .when(mode == current, |d| d.bg(T.hover).text_color(T.text))
                .when(mode != current, |d| d.text_color(T.text3))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.preview_mode = mode;
                    cx.notify();
                }))
                .child(label)
        };

        let toolbar = div()
            .h(px(36.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(T.border)
            .child(div().flex_1().min_w_0().truncate().text_size(px(12.0)).text_color(T.text2).font_family(theme::MONO_FONT).child(file.relative.replace('/', " › ")))
            .child(div().flex().p(px(2.0)).gap(px(2.0)).border_1().border_color(T.border).rounded(px(6.0)).child(mode_button("Fichier", PreviewMode::File, self.preview_mode)).child(mode_button("Diff", PreviewMode::Diff, self.preview_mode)))
            .child(
                div()
                    .id("revert")
                    .h(px(28.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .border_1()
                    .border_color(if self.confirm_revert { T.err } else { T.border })
                    .rounded(px(6.0))
                    .text_size(px(12.0))
                    .text_color(if self.confirm_revert { T.err } else { T.text })
                    .cursor_pointer()
                    .hover(|d| d.bg(T.hover))
                    .on_click(cx.listener(|this, _, _, cx| this.revert_selected(cx)))
                    .child(icon(icons::UNDO).size(px(12.0)))
                    .child(if self.confirm_revert { "Confirmer l'annulation" } else { "Annuler la modification" }),
            )
            .child(
                div()
                    .id("accept")
                    .h(px(28.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded(px(6.0))
                    .bg(T.accent_bg)
                    .text_color(T.accent_on)
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .hover(|d| d.opacity(0.9))
                    .on_click(cx.listener(|this, _, _, cx| this.accept_selected(cx)))
                    .child(icon(icons::CHECK).size(px(12.0)))
                    .child("Accepter"),
            );

        let mode = self.preview_mode;
        let lines = file.lines.iter().filter(|l| mode == PreviewMode::Diff || l.kind != LineKind::Removed).map(|line| {
            let (bg, gutter, sign, sign_color) = match line.kind {
                LineKind::Added => (Some(T.add_bg), Some(T.add_gutter), "+", T.add_fg),
                LineKind::Removed => (Some(T.del_bg), Some(T.del_gutter), "−", T.del_fg),
                LineKind::Same => (None, None, " ", T.text3),
            };
            div()
                .flex()
                .h(px(20.0))
                .when_some(bg, |d, bg| d.bg(bg))
                .child(
                    div()
                        .w(px(48.0))
                        .flex_shrink_0()
                        .pr_2()
                        .flex()
                        .justify_end()
                        .text_color(T.text3)
                        .when_some(gutter, |d, g| d.bg(g))
                        .child(line.number.map(|n| n.to_string()).unwrap_or_default()),
                )
                .child(div().w(px(18.0)).flex_shrink_0().text_center().text_color(sign_color).child(sign))
                .child(div().whitespace_nowrap().child(code_text(&line.text, &line.spans)))
        });

        let files_list = div()
            .flex_shrink_0()
            .max_h(px(180.0))
            .border_t_1()
            .border_color(T.border)
            .bg(T.panel)
            .child(div().px_3().py_2().child(section_label(&format!("Fichiers modifiés dans cette conversation · {}", changed.len()))))
            .child(div().id("changed-files").overflow_y_scroll().max_h(px(140.0)).pb_1().children(changed.iter().enumerate().map(|(i, f)| {
                div()
                    .id(("changed", i))
                    .h(px(26.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .when(i == selected, |d| d.bg(T.hover))
                    .hover(|d| d.bg(T.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(tab) = this.active_tab_mut() {
                            tab.selected_file = i;
                        }
                        cx.notify();
                    }))
                    .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO_FONT).text_size(px(12.0)).child(f.relative.clone()))
                    .child(div().text_size(px(12.0)).font_family(theme::MONO_FONT).text_color(T.add_fg).child(format!("+{}", f.added)))
                    .child(div().text_size(px(12.0)).font_family(theme::MONO_FONT).text_color(T.del_fg).child(format!("−{}", f.removed)))
            })));

        Some(
            panel
                .child(file_tabs)
                .child(toolbar)
                .child(
                    div()
                        .id("code")
                        .flex_1()
                        .min_h_0()
                        .overflow_scroll()
                        .py_2()
                        .font_family(theme::MONO_FONT)
                        .text_size(px(13.0))
                        .line_height(px(20.0))
                        .children(lines),
                )
                .child(files_list),
        )
    }

    /// Onglets du panneau de droite : fichiers de la conversation active, puis exécutions de workflow.
    /// `selected` : fichier affiché (`None` si c'est un workflow qui l'est).
    fn render_right_tabs(&self, selected: Option<usize>, cx: &mut Context<Self>) -> Div {
        let mut tabs = div().flex().items_stretch().flex_1().min_w_0().overflow_hidden();
        let changed = self.active_tab().map(|t| t.changed.clone()).unwrap_or_default();
        for (i, f) in changed.iter().enumerate().take(6) {
            let is_selected = selected == Some(i);
            let name = Path::new(&f.relative).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            tabs = tabs.child(
                div()
                    .id(("file-tab", i))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_3()
                    .border_r_1()
                    .border_color(T.border)
                    .cursor_pointer()
                    .when(is_selected, |d| d.bg(T.surface).border_t_2().border_color(T.accent))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(tab) = this.active_tab_mut() {
                            tab.selected_file = i;
                        }
                        this.right = RightPanel::Code;
                        this.confirm_revert = false;
                        cx.notify();
                    }))
                    .child(div().text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).font_family(theme::MONO_FONT).text_color(rgb(0x7DB8FF)).child(f.extension()))
                    .child(div().text_color(if is_selected { T.text } else { T.text3 }).child(name))
                    .child(div().text_size(px(11.0)).text_color(if f.is_new { T.ok } else { T.warn }).child(if f.is_new { "A" } else { "M" })),
            );
        }
        // Une conversation sans fichier modifié : un onglet « Code » pour revenir à l'aperçu.
        if changed.is_empty() && self.active_tab().is_some() {
            let is_selected = self.right == RightPanel::Code;
            tabs = tabs.child(
                div()
                    .id("code-tab")
                    .flex()
                    .items_center()
                    .px_3()
                    .border_r_1()
                    .border_color(T.border)
                    .cursor_pointer()
                    .when(is_selected, |d| d.bg(T.surface).border_t_2().border_color(T.accent))
                    .text_color(if is_selected { T.text } else { T.text3 })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.right = RightPanel::Code;
                        cx.notify();
                    }))
                    .child("Code"),
            );
        }
        if let Some(opened) = &self.opened {
            let is_selected = self.right == RightPanel::File;
            let name = Path::new(&opened.relative).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            tabs = tabs.child(
                div()
                    .id("opened-tab")
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_3()
                    .border_r_1()
                    .border_color(T.border)
                    .cursor_pointer()
                    .when(is_selected, |d| d.bg(T.surface).border_t_2().border_color(T.accent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.right = RightPanel::File;
                        cx.notify();
                    }))
                    .child(div().text_color(if is_selected { T.text } else { T.text3 }).child(name))
                    .child(
                        div()
                            .id("opened-close")
                            .rounded(px(4.0))
                            .hover(|d| d.bg(T.hover))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.opened = None;
                                if this.right == RightPanel::File {
                                    this.right = RightPanel::Code;
                                }
                                cx.notify();
                            }))
                            .child(icon(icons::CLOSE).size(px(11.0)).text_color(T.text3)),
                    ),
            );
        }
        if let Some(browser) = &self.browser {
            let is_selected = self.right == RightPanel::Browser;
            tabs = tabs.child(
                div()
                    .id("browser-tab")
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_3()
                    .border_r_1()
                    .border_color(T.border)
                    .cursor_pointer()
                    .when(is_selected, |d| d.bg(T.surface).border_t_2().border_color(T.accent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.right = RightPanel::Browser;
                        cx.notify();
                    }))
                    .child(icon(icons::GLOBE).size(px(13.0)).text_color(if is_selected { T.accent } else { T.text3 }))
                    .child(div().max_w(px(160.0)).truncate().text_color(if is_selected { T.text } else { T.text3 }).child(browser.read(cx).title()))
                    .child(
                        div()
                            .id("browser-close")
                            .rounded(px(4.0))
                            .hover(|d| d.bg(T.hover))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_browser(cx);
                            }))
                            .child(icon(icons::CLOSE).size(px(11.0)).text_color(T.text3)),
                    ),
            );
        }
        for run in &self.runs {
            let id = run.id;
            let is_selected = self.right == RightPanel::Run(id);
            let dot = match run.status {
                None => Some((T.accent, !self.pulse)),
                Some(RunStatus::Failed) => Some((T.err, false)),
                _ => None,
            };
            tabs = tabs.child(
                div()
                    .id(("run-tab", id))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_3()
                    .border_r_1()
                    .border_color(T.border)
                    .cursor_pointer()
                    .when(is_selected, |d| d.bg(T.surface).border_t_2().border_color(T.accent))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.right = RightPanel::Run(id);
                        cx.notify();
                    }))
                    .child(icon(icons::WORKFLOW).size(px(13.0)).text_color(if is_selected { T.accent } else { T.text3 }))
                    .child(div().text_color(if is_selected { T.text } else { T.text3 }).child(run.name.clone()))
                    .when_some(dot, |d, (color, dim)| d.child(div().size(px(6.0)).rounded_full().bg(color).when(dim, |d| d.opacity(0.35))))
                    .when(!run.running(), |d| {
                        d.child(
                            div()
                                .id(("run-close", id))
                                .rounded(px(4.0))
                                .hover(|d| d.bg(T.hover))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.runs.retain(|r| r.id != id);
                                    if this.right == RightPanel::Run(id) {
                                        this.right = RightPanel::Code;
                                    }
                                    cx.notify();
                                }))
                                .child(icon(icons::CLOSE).size(px(11.0)).text_color(T.text3)),
                        )
                    }),
            );
        }
        div().h(px(36.0)).flex_shrink_0().flex().border_b_1().border_color(T.border).bg(T.panel).child(tabs)
    }

    fn render_status(&mut self, _cx: &mut Context<Self>) -> impl IntoElement {
        let claude_busy = self.any_active();
        div()
            .h(px(26.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_4()
            .px_3()
            .border_t_1()
            .border_color(T.border)
            .bg(T.panel)
            .text_size(px(12.0))
            .text_color(T.text2)
            .child(match &self.branch {
                Some(branch) => div().flex().items_center().gap_1p5().child(icon(icons::BRANCH).size(px(12.0))).child(branch.clone()),
                None => div().text_color(T.text3).child(if self.current_project().is_some() { "Pas de dépôt git" } else { "Aucun projet ouvert" }),
            })
            .child(div().flex().gap_1().child(div().text_color(T.claude).child("Claude")).child(if !self.claude_installed {
                "non installé"
            } else if claude_busy {
                "● actif"
            } else {
                "○ prêt"
            }))
            .child(div().flex().gap_1().child(div().text_color(T.codex).child("Codex")).child(if self.codex_installed { "○ prêt" } else { "non installé" }))
            .when_some(self.current_run(), |d, run| d.child(div().w(px(1.0)).h(px(14.0)).bg(T.border)).child(self.render_run_status(run)))
            .child(div().flex_1())
            .child(div().text_color(T.text3).child(format!("{} conversation(s) ouverte(s)", self.tabs.len())))
    }
}

fn conversation_row(id: impl Into<ElementId>, title: SharedString, date: Option<String>, selected: bool, pulse: bool, busy: bool) -> Stateful<Div> {
    let dot = |on: bool| div().size(px(4.0)).rounded_full().bg(T.claude).opacity(if on { 1.0 } else { 0.35 });
    div()
        .id(id)
        .h(px(30.0))
        .flex_shrink_0()
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(6.0))
        .cursor_pointer()
        .when(selected, |d| d.bg(T.selected))
        .when(!selected, |d| d.hover(|d| d.bg(T.hover)))
        .child(div().size(px(18.0)).flex_shrink_0().rounded(px(4.0)).bg(T.claude_soft).flex().items_center().justify_center().child(icon(icons::CLAUDE).size(px(12.0)).text_color(T.claude)))
        .child(div().flex_1().min_w_0().truncate().text_color(if selected { T.text } else { T.text2 }).when(selected, |d| d.font_weight(FontWeight::MEDIUM)).child(title))
        .when(busy, |d| d.child(div().flex().gap(px(2.0)).child(dot(pulse)).child(dot(!pulse)).child(dot(pulse))))
        .when_some(date, |d, date| d.child(div().flex_shrink_0().text_size(px(11.0)).text_color(T.text3).child(date)))
}

/// Bordure entre deux panneaux, à glisser pour les redimensionner.
fn splitter(id: &'static str, split: Split, background: Rgba) -> Stateful<Div> {
    div()
        .id(id)
        .group(id)
        .w(px(5.0))
        .h_full()
        .flex_shrink_0()
        .flex()
        .justify_center()
        .bg(background)
        .cursor_col_resize()
        .on_drag(split, |split, _, _, cx| cx.new(|_| *split))
        .child(div().w(px(1.0)).h_full().bg(T.border).group_hover(id, |d| d.bg(T.accent)))
}

fn section_label(text: &str) -> Div {
    div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(T.text3).child(text.to_uppercase())
}

fn tag(text: String) -> Div {
    div().text_size(px(11.0)).line_height(px(16.0)).px_1p5().rounded(px(4.0)).border_1().border_color(T.border).text_color(T.text2).font_family(theme::MONO_FONT).child(text)
}

fn is_active(tab: &Tab) -> bool {
    tab.last_output.is_some_and(|t| t.elapsed() < ACTIVE_WINDOW)
}

fn project_key(project: &Project) -> String {
    project.path.display().to_string().to_lowercase()
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.display().to_string().trim_end_matches(['\\', '/']).eq_ignore_ascii_case(b.display().to_string().trim_end_matches(['\\', '/']))
}

fn which(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| ["", ".exe", ".cmd", ".bat"].iter().any(|ext| dir.join(format!("{program}{ext}")).is_file()))
}

// — Disposition des panneaux (fichier de réglages de SupaCode)

const ACTIVITY_BAR: f32 = 48.0;
const MIN_SIDEBAR: f32 = 200.0;
const MAX_SIDEBAR: f32 = 520.0;
const MIN_RIGHT: f32 = 320.0;
const MIN_CENTER: f32 = 360.0;

fn layout_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("SupaCode").join("disposition.json"))
}

fn load_layout() -> Layout {
    let value = layout_file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let get = |key: &str| value.as_ref().and_then(|v| v.get(key)).and_then(|v| v.as_f64()).map(|v| v as f32);
    let default = Layout::default();
    Layout {
        sidebar: get("barre_laterale").unwrap_or(default.sidebar).clamp(MIN_SIDEBAR, MAX_SIDEBAR),
        right: get("panneau_droit").unwrap_or(default.right).max(MIN_RIGHT),
    }
}

fn save_layout(layout: Layout) {
    let Some(file) = layout_file() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let value = serde_json::json!({ "barre_laterale": layout.sidebar.round(), "panneau_droit": layout.right.round() });
    let _ = std::fs::write(file, serde_json::to_string_pretty(&value).unwrap_or_default());
}

// — Projets ajoutés à la main (fichier de réglages de SupaCode)

fn added_projects_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("SupaCode").join("projets.json"))
}

fn added_projects() -> Vec<PathBuf> {
    added_projects_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|text| serde_json::from_str::<Vec<PathBuf>>(&text).ok())
        .unwrap_or_default()
}

fn remove_added_project(path: &Path) {
    let Some(file) = added_projects_file() else { return };
    let mut projects = added_projects();
    projects.retain(|p| !same_path(p, path));
    let _ = std::fs::write(file, serde_json::to_string_pretty(&projects).unwrap_or_default());
}

fn save_added_project(path: &Path) {
    let Some(file) = added_projects_file() else { return };
    let mut projects = added_projects();
    projects.retain(|p| !same_path(p, path));
    projects.insert(0, path.to_path_buf());
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, serde_json::to_string_pretty(&projects).unwrap_or_default());
}
