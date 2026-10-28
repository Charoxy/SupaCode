//! Tâches : une demande confiée à un ou plusieurs agents qui travaillent seuls, chacun dans
//! sa copie du projet (worktree git). Avec plusieurs agents, on compare leurs résultats et
//! on garde le meilleur ; SupaCode prévient quand c'est fini.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use supacode_core::agents::{self, AgentEvent};
use supacode_core::audit::{self, kind};
use supacode_core::git::{self, FileDiff};
use supacode_core::router::{self, AgentId};

use super::prompts::Pending;
use super::*;

#[derive(Clone, PartialEq)]
pub(super) enum TaskStatus {
    Preparing,
    Running,
    Done,
    Failed(String),
    /// Résultat appliqué au projet.
    Kept,
    /// Copie supprimée.
    Dropped,
}

pub(super) struct AgentTask {
    pub id: usize,
    pub group: usize,
    pub agent: AgentId,
    pub model: Option<String>,
    pub worktree: Option<(PathBuf, String)>,
    pub status: TaskStatus,
    /// Outils utilisés, du plus ancien au plus récent.
    pub activity: Vec<String>,
    pub summary: Option<String>,
    pub files: Vec<FileDiff>,
    pub cost: Option<f64>,
    pub started: Instant,
    pub finished: Option<Instant>,
    pub cancel: Arc<AtomicBool>,
}

impl AgentTask {
    pub fn running(&self) -> bool {
        matches!(self.status, TaskStatus::Preparing | TaskStatus::Running)
    }
}

pub(super) struct TaskGroup {
    pub id: usize,
    pub prompt: String,
    pub root: PathBuf,
    pub created: SystemTime,
}

/// Fichier d'une tâche affiché dans le panneau (diff unifié).
pub(super) struct TaskFile {
    pub task: usize,
    pub path: String,
    pub lines: Vec<(char, String)>,
}

enum TaskMsg {
    Started(PathBuf, String),
    Event(AgentEvent),
    Finished(Result<Vec<FileDiff>, String>),
}

impl Workspace {
    pub(super) fn open_task_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            self.add_project(cx);
            return;
        };
        let choices = vec![
            ("claude".to_owned(), "Claude".to_owned()),
            ("codex".to_owned(), "Codex".to_owned()),
            ("claude+codex".to_owned(), "Comparer Claude et Codex".to_owned()),
            ("claude+claude".to_owned(), "Comparer deux essais de Claude".to_owned()),
        ];
        let subtitle = "L'agent travaille seul dans une copie du projet (worktree git), à partir du dernier commit : tes modifications en cours ne sont pas incluses. Il peut y lancer des commandes (tests…), sauf celles bloquées par les règles d'équipe. SupaCode te prévient à la fin, tu relis le diff et tu gardes le résultat ou non.".to_owned();
        self.ask_user_with(
            format!("Nouvelle tâche · {}", project.name),
            Some(subtitle),
            "Lancer",
            vec![("demande".into(), "Que doit faire l'agent ?".into(), String::new())],
            Some(("Agent", choices)),
            Pending::Task { root: project.path },
            window,
            cx,
        );
    }

    pub(super) fn start_task(&mut self, root: PathBuf, prompt: String, choice: &str, cx: &mut Context<Self>) {
        if prompt.trim().is_empty() {
            self.toast("Décris ce que l'agent doit faire.", cx);
            return;
        }
        let agents: Vec<AgentId> = match choice {
            "codex" => vec![AgentId::Codex],
            "claude+codex" => vec![AgentId::Claude, AgentId::Codex],
            "claude+claude" => vec![AgentId::Claude, AgentId::Claude],
            _ => vec![AgentId::Claude],
        };
        let policy = match self.policy_for(&root) {
            Ok(policy) => policy,
            Err(error) => {
                self.toast(format!("Tâche non lancée, règles d'équipe invalides : {error}"), cx);
                return;
            }
        };
        let group = self.next_task_id;
        self.next_task_id += 1;
        self.task_groups.insert(0, TaskGroup { id: group, prompt: prompt.clone(), root: root.clone(), created: SystemTime::now() });
        audit::record(&root, kind::AGENT, format!("Tâche lancée ({}) : {}", agents.iter().map(|a| a.label()).collect::<Vec<_>>().join(" + "), prompt.chars().take(160).collect::<String>()), serde_json::Value::Null, None);
        let config = router::load_config(&root).unwrap_or_default();
        let extra_settings = self.extension_settings(&root);
        for (n, agent) in agents.into_iter().enumerate() {
            let id = self.next_task_id;
            self.next_task_id += 1;
            let mut decision = router::route(&prompt, &config, Some(agent), None);
            if let Some(Err(message)) = policy.as_ref().map(|p| p.constrain(&mut decision)) {
                self.toast(format!("Tâche refusée par les règles d'équipe : {message}"), cx);
                continue;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            self.tasks.push(AgentTask {
                id,
                group,
                agent: decision.agent,
                model: decision.model.clone(),
                worktree: None,
                status: TaskStatus::Preparing,
                activity: Vec::new(),
                summary: None,
                files: Vec::new(),
                cost: None,
                started: Instant::now(),
                finished: None,
                cancel: cancel.clone(),
            });
            let (tx, rx) = async_channel::unbounded::<TaskMsg>();
            let (root, prompt, model) = (root.clone(), prompt.clone(), decision.model.clone());
            let agent = decision.agent;
            let from_policy = policy.as_ref().and_then(|p| p.claude_settings()).and_then(|s| serde_json::from_str(&s).ok());
            let settings = supacode_core::extensions::merge_settings(from_policy, extra_settings.clone());
            let name = format!("tache-{group}-{}{}", agent.as_str(), if n > 0 { format!("-{n}") } else { String::new() });
            std::thread::spawn(move || {
                let (path, branch) = match git::add_worktree(&root, &name) {
                    Ok(created) => created,
                    Err(e) => {
                        let _ = tx.send_blocking(TaskMsg::Finished(Err(format!("{e:#}"))));
                        return;
                    }
                };
                let _ = tx.send_blocking(TaskMsg::Started(path.clone(), branch));
                // Dans sa copie, l'agent peut lancer des commandes (tests…) ; celles bloquées
                // par les règles d'équipe restent interdites (`--settings`).
                let mut extra: Vec<String> = Vec::new();
                if agent == AgentId::Claude {
                    extra.extend(["--allowedTools".into(), "Bash".into()]);
                    if let Some(s) = settings {
                        extra.extend(["--settings".into(), s.to_string()]);
                    }
                }
                let result = agents::run(agent, model.as_deref(), &prompt, &path, &extra, &cancel, |event| {
                    let _ = tx.send_blocking(TaskMsg::Event(event));
                });
                let outcome = match result {
                    _ if cancel.load(Ordering::Relaxed) => Err("arrêtée".to_owned()),
                    Ok(Some(0)) => git::worktree_diff(&path).map_err(|e| format!("{e:#}")),
                    Ok(code) => Err(format!("l'agent a échoué (code {})", code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()))),
                    Err(e) => Err(format!("{e:#}")),
                };
                let _ = tx.send_blocking(TaskMsg::Finished(outcome));
            });
            let task = cx.spawn(async move |this, cx| {
                while let Ok(msg) = rx.recv().await {
                    let finished = matches!(msg, TaskMsg::Finished(_));
                    if this.update(cx, |this, cx| this.on_task_msg(id, msg, cx)).is_err() || finished {
                        break;
                    }
                }
            });
            self._tasks.push(task);
        }
        self.sidebar_tab = SidebarTab::Tasks;
        self.sidebar_open = true;
        self.right = RightPanel::Tasks(group);
        cx.notify();
    }

    fn on_task_msg(&mut self, id: usize, msg: TaskMsg, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else { return };
        match msg {
            TaskMsg::Started(path, branch) => {
                task.worktree = Some((path, branch));
                task.status = TaskStatus::Running;
            }
            TaskMsg::Event(event) => match event {
                AgentEvent::Tool { name, detail } => {
                    task.activity.push(format!("{name}{}", detail.map(|d| format!(" {}", d.lines().next().unwrap_or(""))).unwrap_or_default()));
                    if task.activity.len() > 200 {
                        task.activity.remove(0);
                    }
                }
                AgentEvent::Text(text) => task.summary = Some(text),
                AgentEvent::Done { summary: Some(summary) } => task.summary = Some(summary),
                AgentEvent::Usage { cost_usd } => task.cost = Some(cost_usd),
                AgentEvent::Error(message) => task.activity.push(format!("✖ {message}")),
                _ => {}
            },
            TaskMsg::Finished(outcome) => {
                task.finished = Some(Instant::now());
                task.status = match outcome {
                    Ok(files) => {
                        task.files = files;
                        TaskStatus::Done
                    }
                    Err(error) => TaskStatus::Failed(error),
                };
                let group = task.group;
                // Toute la tâche est finie : on prévient.
                if !self.tasks.iter().any(|t| t.group == group && t.running()) {
                    let results: Vec<String> = self
                        .tasks
                        .iter()
                        .filter(|t| t.group == group)
                        .map(|t| match &t.status {
                            TaskStatus::Done => format!("{} : {} fichier(s)", t.agent.label(), t.files.len()),
                            _ => format!("{} : échec", t.agent.label()),
                        })
                        .collect();
                    let title = self.task_groups.iter().find(|g| g.id == group).map(|g| g.prompt.lines().next().unwrap_or("").chars().take(50).collect::<String>()).unwrap_or_default();
                    self.toast(format!("Tâche terminée « {title} » · {} · à relire", results.join(", ")), cx);
                    if let Some(g) = self.task_groups.iter().find(|g| g.id == group) {
                        let cost: f64 = self.tasks.iter().filter(|t| t.group == group).filter_map(|t| t.cost).sum();
                        audit::record(&g.root, kind::AGENT, format!("Tâche terminée : {}", results.join(", ")), serde_json::Value::Null, (cost > 0.0).then_some(cost));
                    }
                }
            }
        }
        cx.notify();
    }

    fn stop_task(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some(task) = self.tasks.iter().find(|t| t.id == id) {
            task.cancel.store(true, Ordering::Relaxed);
        }
        cx.notify();
    }

    /// Applique le résultat d'une tâche au projet et supprime les copies du groupe.
    fn keep_task(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|t| t.id == id) else { return };
        let Some(group) = self.task_groups.iter().find(|g| g.id == task.group) else { return };
        let Some((worktree, _)) = task.worktree.clone() else { return };
        let (root, group_id, label) = (group.root.clone(), group.id, task.agent.label());
        match git::apply_worktree(&root, &worktree) {
            Ok(files) => {
                audit::record(&root, kind::AGENT, format!("Résultat de la tâche gardé ({label}, {files} fichier(s))"), serde_json::Value::Null, None);
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
                    task.status = TaskStatus::Kept;
                }
                self.drop_group_worktrees(group_id, cx);
                self.toast(format!("Changements de {label} appliqués au projet ({files} fichier(s)) : relis-les dans le panneau de code avant de committer."), cx);
            }
            Err(e) => self.toast(format!("{e:#}"), cx),
        }
        cx.notify();
    }

    /// Supprime les copies (worktrees et branches) du groupe, sauf rien à garder.
    fn drop_group_worktrees(&mut self, group: usize, cx: &mut Context<Self>) {
        let Some(root) = self.task_groups.iter().find(|g| g.id == group).map(|g| g.root.clone()) else { return };
        for task in self.tasks.iter_mut().filter(|t| t.group == group) {
            task.cancel.store(true, Ordering::Relaxed);
            if let Some((path, branch)) = task.worktree.take() {
                let _ = git::remove_worktree(&root, &path, &branch);
            }
            if task.status != TaskStatus::Kept {
                task.status = TaskStatus::Dropped;
            }
        }
        if self.task_file.as_ref().is_some_and(|f| self.tasks.iter().any(|t| t.id == f.task && t.group == group)) {
            self.task_file = None;
        }
        cx.notify();
    }

    fn show_task_file(&mut self, id: usize, path: String, cx: &mut Context<Self>) {
        if self.task_file.as_ref().is_some_and(|f| f.task == id && f.path == path) {
            self.task_file = None;
            cx.notify();
            return;
        }
        let Some((worktree, _)) = self.tasks.iter().find(|t| t.id == id).and_then(|t| t.worktree.clone()) else { return };
        let diff = git::worktree_file_diff(&worktree, &path).unwrap_or_default();
        let lines = diff
            .lines()
            .skip_while(|l| !l.starts_with("@@"))
            .map(|l| {
                let sign = l.chars().next().unwrap_or(' ');
                (sign, l.get(1..).unwrap_or("").to_owned())
            })
            .collect();
        self.task_file = Some(TaskFile { task: id, path, lines });
        cx.notify();
    }

    // — Rendu

    pub(super) fn render_tasks(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().id("task-list").flex_1().min_h_0().overflow_y_scroll().px_2().pt_1().flex().flex_col().gap(px(2.0));
        if self.task_groups.is_empty() {
            list = list.child(
                div()
                    .m_1()
                    .p_3()
                    .border_1()
                    .border_dashed()
                    .border_color(T.border_strong)
                    .rounded(px(6.0))
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .text_size(px(12.0))
                    .text_color(T.text2)
                    .child(div().font_weight(FontWeight::MEDIUM).text_color(T.text).child("Des agents qui travaillent pendant que tu fais autre chose"))
                    .child("Une tâche tourne seule, dans une copie du projet. Lance la même demande sur Claude et Codex pour comparer leurs diffs et garder le meilleur."),
            );
        }
        let now = SystemTime::now();
        let selected = match self.right {
            RightPanel::Tasks(group) => Some(group),
            _ => None,
        };
        for group in &self.task_groups {
            let id = group.id;
            let chips = self.tasks.iter().filter(|t| t.group == id).map(|t| {
                let (text, color) = task_state(t);
                div().text_size(px(11.0)).px_1p5().rounded(px(4.0)).bg(T.hover).text_color(color).child(format!("{} · {text}", t.agent.label()))
            });
            list = list.child(
                div()
                    .id(("task-group", id))
                    .px_1p5()
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .cursor_pointer()
                    .when(selected == Some(id), |d| d.bg(T.hover))
                    .hover(|d| d.bg(T.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.right = RightPanel::Tasks(id);
                        cx.notify();
                    }))
                    .child(div().flex().gap_2().child(div().flex_1().min_w_0().truncate().child(group.prompt.lines().next().unwrap_or("").to_owned())).child(div().flex_shrink_0().text_size(px(11.0)).text_color(T.text3).child(relative(group.created, now))))
                    .child(div().flex().flex_wrap().gap_1().children(chips)),
            );
        }
        list
    }

    pub(super) fn render_task_panel(&self, group_id: usize, cx: &mut Context<Self>) -> Option<Div> {
        let group = self.task_groups.iter().find(|g| g.id == group_id)?;
        let tasks: Vec<&AgentTask> = self.tasks.iter().filter(|t| t.group == group_id).collect();
        let running = tasks.iter().any(|t| t.running());
        let comparing = tasks.len() > 1;
        let button = |id: (&'static str, usize), label: String, primary: bool| {
            div()
                .id(id)
                .h(px(26.0))
                .px_2p5()
                .flex()
                .items_center()
                .rounded(px(6.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .when(primary, |d| d.bg(T.accent_bg).text_color(T.accent_on).font_weight(FontWeight::MEDIUM).hover(|d| d.opacity(0.9)))
                .when(!primary, |d| d.border_1().border_color(T.border).text_color(T.text2).hover(|d| d.bg(T.hover).text_color(T.text)))
                .child(label)
        };
        let has_copies = tasks.iter().any(|t| t.worktree.is_some());
        let header = div()
            .flex_shrink_0()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_2()
            .border_b_1()
            .border_color(T.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(icons::LAYERS).size(px(14.0)).text_color(T.accent))
                    .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(if comparing { "Comparaison" } else { "Tâche" }))
                    .when(!running && has_copies, |d| {
                        d.child(button(("task-drop", group_id), "Tout abandonner".into(), false).on_click(cx.listener(move |this, _, _, cx| this.drop_group_worktrees(group_id, cx))))
                    })
                    .child(
                        div()
                            .id("task-close")
                            .size(px(26.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .text_color(T.text3)
                            .cursor_pointer()
                            .hover(|d| d.bg(T.hover).text_color(T.text))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.right = RightPanel::Code;
                                cx.notify();
                            }))
                            .child(icon(icons::CLOSE).size(px(12.0))),
                    ),
            )
            .child(div().text_size(px(12.0)).text_color(T.text2).child(group.prompt.chars().take(600).collect::<String>()));

        let cards = tasks.iter().map(|task| {
            let id = task.id;
            let (state, color) = task_state(task);
            let elapsed = task.finished.unwrap_or_else(Instant::now).saturating_duration_since(task.started).as_secs();
            let added: usize = task.files.iter().map(|f| f.added).sum();
            let removed: usize = task.files.iter().map(|f| f.removed).sum();
            let open_file = self.task_file.as_ref().filter(|f| f.task == id);
            let files = task.files.iter().enumerate().map(|(i, f)| {
                let path = f.path.clone();
                let is_open = open_file.is_some_and(|o| o.path == f.path);
                div()
                    .id(SharedString::from(format!("task-{id}-file-{i}")))
                    .h(px(24.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .when(is_open, |d| d.bg(T.hover))
                    .hover(|d| d.bg(T.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.show_task_file(id, path.clone(), cx)))
                    .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO_FONT).text_size(px(12.0)).child(f.path.clone()))
                    .child(div().font_family(theme::MONO_FONT).text_size(px(11.0)).text_color(T.add_fg).child(format!("+{}", f.added)))
                    .child(div().font_family(theme::MONO_FONT).text_size(px(11.0)).text_color(T.del_fg).child(format!("−{}", f.removed)))
            });
            let diff = open_file.map(|file| {
                div()
                    .id(SharedString::from(format!("task-{id}-diff")))
                    .max_h(px(320.0))
                    .overflow_scroll()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(T.border_subtle)
                    .bg(T.app)
                    .py_1()
                    .font_family(theme::MONO_FONT)
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .children(file.lines.iter().map(|(sign, text)| {
                        let (bg, fg) = match sign {
                            '+' => (Some(T.add_bg), T.add_fg),
                            '-' => (Some(T.del_bg), T.del_fg),
                            '@' => (None, T.accent),
                            _ => (None, T.text3),
                        };
                        div().flex().px_2().when_some(bg, |d, bg| d.bg(bg)).child(div().w(px(14.0)).flex_shrink_0().text_color(fg).child(if *sign == '@' { String::new() } else { sign.to_string() })).child(div().whitespace_nowrap().child(if text.is_empty() { " ".to_owned() } else { text.replace('\t', "    ") }))
                    }))
            });
            let last_activity = task.activity.iter().rev().take(4).rev().map(|a| div().truncate().child(a.clone()));
            let actions = div()
                .flex()
                .gap_1p5()
                .when(task.running(), |d| d.child(button(("task-stop", id), "Arrêter".into(), false).on_click(cx.listener(move |this, _, _, cx| this.stop_task(id, cx)))))
                .when(task.status == TaskStatus::Done && !task.files.is_empty(), |d| {
                    d.child(button(("task-keep", id), if comparing { "Garder ce résultat".into() } else { "Appliquer au projet".into() }, true).on_click(cx.listener(move |this, _, _, cx| this.keep_task(id, cx))))
                })
                .when_some(task.worktree.clone().filter(|_| !task.running()), |d, (path, _)| {
                    d.child(button(("task-folder", id), "Ouvrir la copie".into(), false).on_click(move |_, _, _| {
                        let _ = std::process::Command::new("explorer").arg(&path).spawn();
                    }))
                });
            div()
                .p_3()
                .rounded(px(8.0))
                .border_1()
                .border_color(if task.status == TaskStatus::Kept { T.ok } else { T.border })
                .bg(T.surface)
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().font_weight(FontWeight::MEDIUM).child(task.agent.label()))
                        .when_some(task.model.clone(), |d, m| d.child(tag(m)))
                        .child(div().flex_1())
                        .child(div().text_size(px(12.0)).text_color(color).child(state))
                        .child(div().text_size(px(11.0)).font_family(theme::MONO_FONT).text_color(T.text3).child(format!("{}:{:02}", elapsed / 60, elapsed % 60)))
                        .when_some(task.cost, |d, c| d.child(div().text_size(px(11.0)).text_color(T.text3).child(format!("{c:.2} $").replace('.', ",")))),
                )
                .when(task.running(), |d| d.child(div().flex().flex_col().text_size(px(11.0)).text_color(T.text3).font_family(theme::MONO_FONT).children(last_activity)))
                .when_some(task.summary.clone().filter(|_| !task.running()), |d, s| d.child(div().text_size(px(12.0)).text_color(T.text2).child(s.chars().take(700).collect::<String>())))
                .when(!task.files.is_empty(), |d| {
                    d.child(div().text_size(px(11.0)).text_color(T.text3).child(format!("{} fichier(s) · +{added} −{removed}", task.files.len()))).child(div().flex().flex_col().children(files))
                })
                .when(task.status == TaskStatus::Done && task.files.is_empty(), |d| d.child(div().text_size(px(12.0)).text_color(T.text3).child("Aucun fichier modifié.")))
                .children(diff)
                .child(actions)
        });
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(header)
                .child(div().id("task-cards").flex_1().min_h_0().overflow_y_scroll().p_3().flex().flex_col().gap_3().children(cards)),
        )
    }
}

fn task_state(task: &AgentTask) -> (String, Rgba) {
    match &task.status {
        TaskStatus::Preparing => ("préparation".into(), T.accent),
        TaskStatus::Running => ("en cours".into(), T.accent),
        TaskStatus::Done => (if task.files.is_empty() { "fini, rien modifié".into() } else { format!("{} fichier(s) à relire", task.files.len()) }, T.ok),
        TaskStatus::Failed(e) => (format!("échec : {}", e.chars().take(60).collect::<String>()), T.err),
        TaskStatus::Kept => ("gardé".into(), T.ok),
        TaskStatus::Dropped => ("abandonné".into(), T.text3),
    }
}
