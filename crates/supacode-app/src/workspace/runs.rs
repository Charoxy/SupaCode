//! Workflows lancés depuis l'IDE : exécution en arrière-plan (moteur de `supacode-core`),
//! suivi étape par étape dans le panneau de droite et cartes de validation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use supacode_core::agents::AgentEvent;
use supacode_core::engine::{self, Event, Host, RunOptions, RunStatus, StepKind, StepStatus};
use supacode_core::process::{self, Spawn};
use supacode_core::router::AgentId;
use supacode_core::workflows::{Step, WorkflowFile};

use super::*;

/// Lignes conservées par étape.
const MAX_LINES: usize = 400;
/// Lignes visibles dans le cadre d'une étape repliée sur sa fin.
const VISIBLE_LINES: usize = 12;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum LineStyle {
    Plain,
    Muted,
    Done,
    Error,
}

pub(super) struct StepView {
    id: String,
    name: String,
    kind: StepKind,
    /// Commande, ou agent forcé.
    detail: Option<String>,
    depth: usize,
    started: Option<Instant>,
    finished: Option<Instant>,
    status: Option<StepStatus>,
    exit_code: Option<i32>,
    agent: Option<(AgentId, Option<String>)>,
    lines: Vec<(LineStyle, String)>,
    /// Choix de l'utilisateur ; sinon ouvert pendant l'exécution et en cas d'échec.
    expanded: Option<bool>,
}

impl StepView {
    fn running(&self) -> bool {
        self.started.is_some() && self.status.is_none()
    }

    fn is_expanded(&self) -> bool {
        self.expanded.unwrap_or(self.running() || self.status == Some(StepStatus::Failed)) && !self.lines.is_empty()
    }

    fn push(&mut self, style: LineStyle, text: String) {
        for line in text.lines() {
            self.lines.push((style, line.to_owned()));
        }
        if self.lines.len() > MAX_LINES {
            let extra = self.lines.len() - MAX_LINES;
            self.lines.drain(..extra);
        }
    }
}

pub(super) struct Approval {
    step: String,
    message: String,
    reply: mpsc::Sender<bool>,
    /// Fichiers modifiés et lignes +/− au moment de la demande.
    stats: Option<(usize, usize, usize)>,
}

pub(super) struct WorkflowRun {
    pub(super) id: usize,
    pub(super) workflow_id: String,
    pub(super) name: String,
    pub(super) project: PathBuf,
    branch: Option<String>,
    started: Instant,
    pub(super) status: Option<RunStatus>,
    cancel: Arc<AtomicBool>,
    steps: Vec<StepView>,
    approval: Option<Approval>,
}

impl WorkflowRun {
    pub(super) fn running(&self) -> bool {
        self.status.is_none()
    }

    /// Étapes de premier niveau terminées / total.
    pub(super) fn progress(&self) -> (usize, usize) {
        let top: Vec<_> = self.steps.iter().filter(|s| s.depth == 0).collect();
        (top.iter().filter(|s| s.status.is_some()).count(), top.len())
    }

    fn step_mut(&mut self, id: &str) -> Option<&mut StepView> {
        self.steps.iter_mut().find(|s| s.id == id)
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::StepStart { id } => {
                if let Some(step) = self.step_mut(&id) {
                    step.started = Some(Instant::now());
                }
            }
            Event::Routed { id, agent, model, reason } => {
                if let Some(step) = self.step_mut(&id) {
                    step.agent = Some((agent, model));
                    step.push(LineStyle::Muted, format!("orchestrateur : {reason}"));
                }
            }
            Event::Output { id, event } => {
                if let Some(step) = self.step_mut(&id) {
                    match event {
                        AgentEvent::Text(text) => step.push(LineStyle::Plain, text),
                        AgentEvent::Tool { name, detail } => step.push(LineStyle::Done, format!("✓ {name}{}", detail.map(|d| format!(" {d}")).unwrap_or_default())),
                        AgentEvent::Log(text) => step.push(if step.kind == StepKind::Run { LineStyle::Plain } else { LineStyle::Muted }, text),
                        AgentEvent::Error(text) => step.push(LineStyle::Error, text),
                        AgentEvent::Done { .. } => {}
                    }
                }
            }
            // La carte de validation arrive avec sa réponse attendue (`Msg::Approval`).
            Event::Approval { .. } => {}
            Event::StepDone { id, status, exit_code } => {
                if self.approval.as_ref().is_some_and(|a| a.step == id) {
                    self.approval = None;
                }
                if let Some(step) = self.step_mut(&id) {
                    step.started.get_or_insert_with(Instant::now);
                    step.finished = Some(Instant::now());
                    step.status = Some(status);
                    step.exit_code = exit_code;
                }
            }
            Event::Done { status } => {
                self.status = Some(status);
                self.approval = None;
            }
        }
    }
}

enum Msg {
    Event(Event),
    Approval { step: String, message: String, reply: mpsc::Sender<bool> },
}

struct ChannelHost(async_channel::Sender<Msg>);

impl Host for ChannelHost {
    fn emit(&self, event: Event) {
        let _ = self.0.send_blocking(Msg::Event(event));
    }

    fn approve(&self, id: &str, message: &str, cancel: &AtomicBool) -> bool {
        let (reply, answer) = mpsc::channel();
        if self.0.send_blocking(Msg::Approval { step: id.to_owned(), message: message.to_owned(), reply }).is_err() {
            return false;
        }
        loop {
            match answer.recv_timeout(Duration::from_millis(200)) {
                Ok(approved) => return approved,
                Err(mpsc::RecvTimeoutError::Disconnected) => return false,
                Err(mpsc::RecvTimeoutError::Timeout) if cancel.load(Ordering::Relaxed) => return false,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

fn step_views(steps: &[Step], depth: usize, out: &mut Vec<StepView>) {
    for step in steps {
        let kind = step.kind();
        let detail = match kind {
            StepKind::Run => step.run.clone(),
            StepKind::Agent => step.agent.clone().filter(|a| a != "auto"),
            _ => None,
        };
        out.push(StepView {
            id: step.id.clone(),
            name: step.label().to_owned(),
            kind,
            detail,
            depth,
            started: None,
            finished: None,
            status: None,
            exit_code: None,
            agent: None,
            lines: Vec::new(),
            expanded: None,
        });
        if let Some(children) = &step.parallel {
            step_views(children, depth + 1, out);
        }
    }
}

/// Fichiers modifiés et lignes ajoutées / supprimées par rapport à HEAD.
fn diff_stats(root: &Path) -> Option<(usize, usize, usize)> {
    let (mut files, mut added, mut removed) = (0, 0, 0);
    let spawn = Spawn::program("git", ["diff", "--numstat", "HEAD"].map(Into::into).into(), root);
    let code = process::run_lines(spawn, &AtomicBool::new(false), |line| {
        let mut parts = line.text.split('\t');
        if let (Some(a), Some(r), Some(_)) = (parts.next(), parts.next(), parts.next()) {
            files += 1;
            added += a.parse::<usize>().unwrap_or(0);
            removed += r.parse::<usize>().unwrap_or(0);
        }
    })
    .ok()??;
    (code == 0).then_some((files, added, removed))
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f32();
    if secs < 60.0 { format!("{secs:.1} s").replace('.', ",") } else { format!("{}:{:02}", d.as_secs() / 60, d.as_secs() % 60) }
}

fn ago(d: Duration) -> String {
    match d.as_secs() {
        s if s < 60 => format!("il y a {s} s"),
        s if s < 3600 => format!("il y a {} min", s / 60),
        s => format!("il y a {} h", s / 3600),
    }
}

impl Workspace {
    /// Lance le workflow dans le projet ; le panneau de droite affiche son suivi.
    pub(super) fn run_workflow(&mut self, project: PathBuf, file: WorkflowFile, cx: &mut Context<Self>) {
        // Une seule exécution par workflow : l'ancienne (terminée) est remplacée.
        if self.runs.iter().any(|r| r.workflow_id == file.id && same_path(&r.project, &project) && r.running()) {
            return;
        }
        self.runs.retain(|r| !(r.workflow_id == file.id && same_path(&r.project, &project)));
        let id = self.next_run_id;
        self.next_run_id += 1;
        let mut steps = Vec::new();
        step_views(&file.workflow.steps, 0, &mut steps);
        let cancel = Arc::new(AtomicBool::new(false));
        self.runs.push(WorkflowRun {
            id,
            workflow_id: file.id.clone(),
            name: file.workflow.name.clone(),
            project: project.clone(),
            branch: preview::git_branch(&project),
            started: Instant::now(),
            status: None,
            cancel: cancel.clone(),
            steps,
            approval: None,
        });
        self.right = RightPanel::Run(id);

        let (tx, rx) = async_channel::unbounded();
        let workflow = file.workflow;
        let options = RunOptions { cwd: project.clone(), context: HashMap::new(), cancel };
        std::thread::spawn(move || {
            engine::run(&workflow, &options, &ChannelHost(tx));
        });

        let task = cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                // Une commande bavarde envoie beaucoup de lignes : un seul rendu par lot.
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                let mut wants_stats = false;
                let alive = this.update(cx, |this, cx| {
                    let Some(run) = this.runs.iter_mut().find(|r| r.id == id) else { return };
                    for msg in batch {
                        match msg {
                            Msg::Event(event) => run.apply(event),
                            Msg::Approval { step, message, reply } => {
                                run.approval = Some(Approval { step, message, reply, stats: None });
                                wants_stats = true;
                            }
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
                if wants_stats {
                    let root = project.clone();
                    let stats = cx.background_executor().spawn(async move { diff_stats(&root) }).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(approval) = this.runs.iter_mut().find(|r| r.id == id).and_then(|r| r.approval.as_mut()) {
                            approval.stats = stats;
                            cx.notify();
                        }
                    });
                }
            }
        });
        self._tasks.push(task);
        cx.notify();
    }

    pub(super) fn stop_run(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some(run) = self.runs.iter_mut().find(|r| r.id == id) {
            run.cancel.store(true, Ordering::Relaxed);
            cx.notify();
        }
    }

    fn answer_approval(&mut self, id: usize, approved: bool, cx: &mut Context<Self>) {
        if let Some(approval) = self.runs.iter_mut().find(|r| r.id == id).and_then(|r| r.approval.take()) {
            let _ = approval.reply.send(approved);
            cx.notify();
        }
    }

    pub(super) fn run_for(&self, project: &Path, workflow_id: &str) -> Option<&WorkflowRun> {
        self.runs.iter().find(|r| r.workflow_id == workflow_id && same_path(&r.project, project))
    }

    /// Exécution en cours la plus récente, pour la barre d'état.
    pub(super) fn current_run(&self) -> Option<&WorkflowRun> {
        self.runs.iter().rev().find(|r| r.running())
    }

    /// Barre de progression de la barre d'état : un segment par étape de premier niveau.
    pub(super) fn render_run_status(&self, run: &WorkflowRun) -> Div {
        let (done, total) = run.progress();
        let segments = run.steps.iter().filter(|s| s.depth == 0).map(|s| {
            let (color, pulse) = match s.status {
                Some(StepStatus::Success) => (T.ok, false),
                Some(StepStatus::Failed) => (T.err, false),
                Some(StepStatus::Skipped) => (T.border_strong, false),
                None if s.running() => (T.accent, true),
                None => (T.border_strong, false),
            };
            div().w(px(18.0)).h(px(4.0)).rounded(px(2.0)).bg(color).when(pulse && !self.pulse, |d| d.opacity(0.4))
        });
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(icon(icons::WORKFLOW).size(px(13.0)).text_color(T.accent))
            .child(run.name.clone())
            .child(div().flex().gap(px(2.0)).children(segments))
            .child(div().text_color(T.text3).child(format!("{}/{total}", (done + 1).min(total))))
    }

    /// Panneau de droite : suivi de l'exécution.
    pub(super) fn render_run(&self, id: usize, cx: &mut Context<Self>) -> Option<Div> {
        let run = self.runs.iter().find(|r| r.id == id)?;
        let pulse = if self.pulse { 1.0 } else { 0.35 };

        let header = div()
            .px_4()
            .py_3()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(T.border_subtle)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).child(run.name.clone()))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .text_size(px(12.0))
                            .text_color(T.text3)
                            .child("Déclenché par")
                            .child(div().font_family(theme::MONO_FONT).text_color(T.text2).child("manuel"))
                            .children(run.branch.clone().map(|b| format!("· {b}")))
                            .child(format!("· {}", ago(run.started.elapsed()))),
                    ),
            )
            .child(if run.running() {
                div()
                    .id("run-stop")
                    .h(px(28.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .border_1()
                    .border_color(T.border)
                    .rounded(px(6.0))
                    .text_size(px(12.0))
                    .text_color(T.err)
                    .cursor_pointer()
                    .hover(|d| d.bg(T.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.stop_run(id, cx)))
                    .child(icon(icons::STOP).size(px(10.0)))
                    .child(if run.cancel.load(Ordering::Relaxed) { "Arrêt…" } else { "Arrêter" })
                    .into_any_element()
            } else {
                let (label, color) = match run.status {
                    Some(RunStatus::Success) => ("Terminé", T.ok),
                    Some(RunStatus::Failed) => ("Échec", T.err),
                    _ => ("Arrêté", T.text3),
                };
                let (project, workflow_id) = (run.project.clone(), run.workflow_id.clone());
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().text_size(px(12.0)).text_color(color).child(label))
                    .child(
                        div()
                            .id("run-again")
                            .h(px(28.0))
                            .px(px(10.0))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .border_1()
                            .border_color(T.border)
                            .rounded(px(6.0))
                            .bg(T.raised)
                            .text_size(px(12.0))
                            .cursor_pointer()
                            .hover(|d| d.bg(T.hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let file = load_workflows(&project).ok().and_then(|files| files.into_iter().find(|f| f.id == workflow_id));
                                if let Some(file) = file {
                                    this.run_workflow(project.clone(), file, cx);
                                }
                            }))
                            .child(icon(icons::PLAY).size(px(10.0)))
                            .child("Relancer"),
                    )
                    .into_any_element()
            });

        let mut timeline = div().id("run-steps").flex_1().min_h_0().overflow_y_scroll().p_4().flex().flex_col();
        let count = run.steps.len();
        for (index, step) in run.steps.iter().enumerate() {
            timeline = timeline.child(self.render_step(run, index, step, index + 1 == count, pulse, cx));
            if run.approval.as_ref().is_some_and(|a| a.step == step.id) {
                timeline = timeline.child(self.render_approval(run, step, cx));
            }
        }

        Some(div().flex_1().min_h_0().flex().flex_col().child(header).child(timeline))
    }

    fn render_step(&self, run: &WorkflowRun, index: usize, step: &StepView, last: bool, pulse: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let agent_color = match step.agent.as_ref().map(|(a, _)| *a).or(match step.detail.as_deref() {
            Some("codex") if step.kind == StepKind::Agent => Some(AgentId::Codex),
            Some("claude") if step.kind == StepKind::Agent => Some(AgentId::Claude),
            _ => None,
        }) {
            Some(AgentId::Codex) => (T.codex, T.codex_soft),
            _ => (T.claude, T.claude_soft),
        };
        let accent = if step.kind == StepKind::Agent { agent_color.0 } else { T.accent };

        let marker = match step.status {
            Some(StepStatus::Success) => div().size(px(22.0)).rounded_full().bg(T.ok_soft).text_color(T.ok).flex().items_center().justify_center().child(icon(icons::CHECK).size(px(12.0))),
            Some(StepStatus::Failed) => div().size(px(22.0)).rounded_full().bg(T.err_soft).text_color(T.err).flex().items_center().justify_center().child(icon(icons::CLOSE).size(px(11.0))),
            Some(StepStatus::Skipped) => div().size(px(22.0)).rounded_full().border_1().border_color(T.border).text_color(T.text3).flex().items_center().justify_center().child("–"),
            None if step.running() => div()
                .size(px(22.0))
                .rounded_full()
                .border_2()
                .border_color(accent)
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(px(8.0)).rounded_full().bg(accent).opacity(pulse)),
            None => div().size(px(22.0)).rounded_full().border_1().border_dashed().border_color(T.border_strong),
        };

        let (state, state_color) = match step.status {
            Some(StepStatus::Success) => ("Réussi".to_owned(), T.ok),
            Some(StepStatus::Failed) if run.status == Some(RunStatus::Cancelled) && step.exit_code.is_none() => ("Arrêté".to_owned(), T.text3),
            Some(StepStatus::Failed) => match (step.kind, step.exit_code) {
                (StepKind::Approve, _) => ("Refusé".to_owned(), T.err),
                (_, Some(code)) => (format!("Échec · code {code}"), T.err),
                _ => ("Échec".to_owned(), T.err),
            },
            Some(StepStatus::Skipped) => ("Ignoré".to_owned(), T.text3),
            None if step.running() && step.kind == StepKind::Approve => ("En attente de validation".to_owned(), T.accent),
            None if step.running() => ("En cours".to_owned(), accent),
            None if run.running() => ("En attente".to_owned(), T.text3),
            None => ("Non lancé".to_owned(), T.text3),
        };
        let elapsed = step.started.map(|s| step.finished.unwrap_or_else(Instant::now).saturating_duration_since(s)).filter(|_| step.status != Some(StepStatus::Skipped));

        let tag = match (step.kind, &step.agent, &step.detail) {
            (StepKind::Agent, Some((agent, model)), _) => Some(format!("{}{}", agent.label(), model.as_ref().map(|m| format!(" · {m}")).unwrap_or_default())),
            (StepKind::Agent, None, Some(agent)) => Some(agent.clone()),
            (StepKind::Agent, None, None) => Some("auto".to_owned()),
            (StepKind::Parallel, ..) => Some("en parallèle".to_owned()),
            _ => None,
        };
        let expanded = step.is_expanded();
        let (run_id, step_id) = (run.id, step.id.clone());
        let waiting = step.status.is_none() && !step.running();

        let title = div()
            .id(("step", index))
            .h(px(22.0))
            .flex()
            .items_center()
            .gap_2()
            .when(!step.lines.is_empty(), |d| {
                d.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(step) = this.runs.iter_mut().find(|r| r.id == run_id).and_then(|r| r.step_mut(&step_id)) {
                        step.expanded = Some(!expanded);
                    }
                    cx.notify();
                }))
            })
            .child(icon(if expanded { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT }).size(px(12.0)).text_color(if step.lines.is_empty() { T.border_strong } else { T.text3 }))
            .child(div().font_weight(FontWeight::MEDIUM).text_color(if waiting { T.text2 } else { T.text }).child(step.name.clone()))
            .when_some(step.detail.clone().filter(|_| step.kind == StepKind::Run), |d, command| {
                d.child(div().min_w_0().truncate().font_family(theme::MONO_FONT).text_size(px(12.0)).text_color(T.text3).child(command))
            })
            .when_some(tag, |d, tag| {
                let (fg, bg) = if step.kind == StepKind::Agent { agent_color } else { (T.text2, T.hover) };
                d.child(div().flex_shrink_0().text_size(px(11.0)).px_1p5().rounded(px(4.0)).bg(bg).text_color(fg).child(tag))
            })
            .child(div().flex_1())
            .child(div().flex_shrink_0().text_size(px(12.0)).text_color(state_color).child(state))
            .when_some(elapsed, |d, e| d.child(div().flex_shrink_0().text_size(px(12.0)).font_family(theme::MONO_FONT).text_color(T.text3).child(format_duration(e))));

        let output = expanded.then(|| {
            let start = step.lines.len().saturating_sub(VISIBLE_LINES);
            let lines = step.lines[start..].iter().enumerate().map(|(i, (style, text))| {
                let color = match style {
                    LineStyle::Plain => T.text2,
                    LineStyle::Muted => T.text3,
                    LineStyle::Done => T.ok,
                    LineStyle::Error => T.err,
                };
                let live = step.running() && start + i + 1 == step.lines.len();
                div().flex().gap_1p5().child(div().text_color(color).whitespace_nowrap().child(if text.is_empty() { " ".to_owned() } else { text.replace('\t', "    ") })).when(live, |d| d.child(div().text_color(accent).opacity(pulse).child("›")))
            });
            div()
                .border_1()
                .border_color(T.border_subtle)
                .rounded(px(6.0))
                .bg(T.app)
                .px_3()
                .py_2()
                .overflow_hidden()
                .font_family(theme::MONO_FONT)
                .text_size(px(12.0))
                .line_height(px(18.0))
                .when(start > 0, |d| d.child(div().text_color(T.text3).child(format!("… {start} ligne(s) plus haut"))))
                .children(lines)
        });

        div()
            .flex()
            .gap_3()
            .when(step.depth > 0, |d| d.pl(px(34.0 * step.depth as f32)))
            .child(
                div()
                    .w(px(22.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(marker)
                    .when(!last, |d| d.child(div().flex_1().w(px(1.0)).min_h(px(12.0)).bg(T.border))),
            )
            .child(div().flex_1().min_w_0().pb_3().flex().flex_col().gap_2().child(title).children(output))
    }

    fn render_approval(&self, run: &WorkflowRun, step: &StepView, cx: &mut Context<Self>) -> impl IntoElement {
        let approval = run.approval.as_ref().expect("validation en attente");
        let id = run.id;
        let button = |label: &'static str, primary: bool, approved: bool| {
            div()
                .id(label)
                .h(px(30.0))
                .px(px(14.0))
                .flex()
                .items_center()
                .rounded(px(6.0))
                .cursor_pointer()
                .when(primary, |d| d.bg(T.accent_bg).text_color(T.accent_on).font_weight(FontWeight::MEDIUM).hover(|d| d.opacity(0.9)))
                .when(!primary, |d| d.border_1().border_color(T.border).hover(|d| d.bg(T.hover)))
                .on_click(cx.listener(move |this, _, _, cx| this.answer_approval(id, approved, cx)))
                .child(label)
        };
        div()
            .ml(px(34.0 + 34.0 * step.depth as f32))
            .mb_3()
            .border_1()
            .border_color(T.accent)
            .rounded(px(6.0))
            .bg(T.raised)
            .px_4()
            .py(px(14.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon(icons::APPROVE).size(px(16.0)).text_color(T.accent))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(14.0)).child(step.name.clone()))
                    .child(div().flex_1())
                    .child(div().text_size(px(11.0)).text_color(T.text3).child("validation requise")),
            )
            .child(div().font_family(theme::MONO_FONT).text_size(px(12.0)).px(px(10.0)).py_2().rounded(px(4.0)).bg(T.app).child(approval.message.clone()))
            .when_some(approval.stats, |d, (files, added, removed)| {
                d.child(
                    div()
                        .flex()
                        .gap_3()
                        .text_size(px(12.0))
                        .text_color(T.text2)
                        .child(format!("{files} fichier(s) modifié(s)"))
                        .child(div().flex().gap_1().font_family(theme::MONO_FONT).child(div().text_color(T.add_fg).child(format!("+{added}"))).child(div().text_color(T.del_fg).child(format!("−{removed}")))),
                )
            })
            .child(div().flex().gap_2().justify_end().child(button("Refuser", false, false)).child(button("Valider", true, true)))
    }
}
