//! Fenêtre « Nouveau workflow » : l'utilisateur décrit ce qu'il veut, Claude propose le workflow
//! (aperçu en blocs ou en JSON), puis le fichier est créé dans `.supacode/workflows`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use supacode_core::agents::AgentEvent;
use supacode_core::generate::{self, Generated};
use supacode_core::workflows::{Step, Trigger, Workflow, save_workflow};

use crate::highlight::{self, code_text};
use crate::icons::{self, icon};
use crate::theme::{self, DARK as T};

const EXAMPLES: [&str; 3] = [
    "À chaque sauvegarde d'un fichier .ts, lance ESLint sur ce fichier.",
    "Avant un push, demande à Claude une review du diff et bloque si elle trouve un problème.",
    "À chaque commit, lance les tests ; s'ils échouent, demande à Codex de corriger puis relance-les, et je valide à la fin.",
];

enum State {
    Idle,
    Generating { activity: Vec<String>, cancel: Arc<AtomicBool> },
    Ready(Generated),
    Failed(String),
}

pub enum CreatorEvent {
    /// Workflow enregistré sous cet id.
    Created(String),
    /// Ouvrir le workflow proposé dans l'éditeur visuel, sans l'enregistrer.
    Edit(Workflow),
    Dismiss,
}

impl EventEmitter<CreatorEvent> for Creator {}

pub struct Creator {
    project: PathBuf,
    project_name: String,
    description: Entity<TextareaState>,
    name: Entity<InputState>,
    state: State,
    show_json: bool,
    save_error: Option<String>,
    _tasks: Vec<Task<()>>,
}

impl Creator {
    pub fn new(project: PathBuf, project_name: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let description = cx.new(|cx| TextareaState::new(window, cx).rows(7).placeholder("Ex. : à chaque commit, lance le lint puis les tests, et demande-moi de valider."));
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Nom du workflow"));
        description.update(cx, |d, cx| d.focus(window, cx));
        Self { project, project_name, description, name, state: State::Idle, show_json: false, save_error: None, _tasks: Vec::new() }
    }

    fn generate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.description.read(cx).value().trim().to_owned();
        if text.is_empty() || matches!(self.state, State::Generating { .. }) {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.state = State::Generating { activity: Vec::new(), cancel: cancel.clone() };
        self.save_error = None;
        let (tx, rx) = async_channel::unbounded::<String>();
        let cwd = self.project.clone();
        let job = cx.background_executor().spawn(async move {
            generate::generate(&text, &cwd, &cancel, |event| {
                if let AgentEvent::Tool { name, detail } = event {
                    let _ = tx.send_blocking(format!("{name}{}", detail.map(|d| format!(" {d}")).unwrap_or_default()));
                }
            })
        });
        // Activité de Claude (fichiers lus) pendant la génération.
        let activity = cx.spawn(async move |this, cx| {
            while let Ok(line) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    if let State::Generating { activity, .. } = &mut this.state {
                        activity.push(line);
                        cx.notify();
                    }
                });
            }
        });
        let result = cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if !matches!(this.state, State::Generating { .. }) {
                    return;
                }
                this.state = match result {
                    Ok(generated) => {
                        let name = generated.workflow.name.clone();
                        this.name.update(cx, |input, cx| input.set_value(name, window, cx));
                        State::Ready(generated)
                    }
                    Err(err) => State::Failed(format!("{err:#}")),
                };
                cx.notify();
            });
        });
        self._tasks.extend([activity, result]);
        cx.notify();
    }

    /// Remplit la description et lance la génération (tests visuels).
    #[cfg(debug_assertions)]
    pub fn debug_generate(&mut self, description: &str, window: &mut Window, cx: &mut Context<Self>) {
        let description = description.to_owned();
        self.description.update(cx, |d, cx| d.set_value(description, window, cx));
        self.generate(window, cx);
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let State::Ready(generated) = &self.state else { return };
        let mut workflow = generated.workflow.clone();
        let name = self.name.read(cx).value().trim().to_owned();
        if !name.is_empty() {
            workflow.name = name;
        }
        match save_workflow(&self.project, &generate::slug(&workflow.name), &workflow) {
            Ok(id) => cx.emit(CreatorEvent::Created(id)),
            Err(err) => {
                self.save_error = Some(format!("{err:#}"));
                cx.notify();
            }
        }
    }

    fn edit(&mut self, cx: &mut Context<Self>) {
        let State::Ready(generated) = &self.state else { return };
        let mut workflow = generated.workflow.clone();
        let name = self.name.read(cx).value().trim().to_owned();
        if !name.is_empty() {
            workflow.name = name;
        }
        cx.emit(CreatorEvent::Edit(workflow));
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if let State::Generating { cancel, .. } = &self.state {
            cancel.store(true, Ordering::Relaxed);
        }
        cx.emit(CreatorEvent::Dismiss);
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.key == "enter" && k.modifiers.control {
            self.generate(window, cx);
            cx.stop_propagation();
        } else if k.key == "escape" {
            self.dismiss(cx);
            cx.stop_propagation();
        }
    }
}

impl Render for Creator {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let generating = matches!(self.state, State::Generating { .. });
        let ready = matches!(self.state, State::Ready(_));

        let header = div()
            .h(px(52.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px_5()
            .border_b_1()
            .border_color(T.border)
            .child(icon(icons::WORKFLOW).size(px(16.0)).text_color(T.accent))
            .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).child("Nouveau workflow"))
            .child(div().text_size(px(12.0)).text_color(T.text3).child(self.project_name.clone()))
            .child(div().flex_1())
            .child(
                div()
                    .id("creator-close")
                    .size(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .text_color(T.text2)
                    .cursor_pointer()
                    .hover(|d| d.bg(T.hover))
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx)))
                    .child(icon(icons::CLOSE).size(px(14.0))),
            );

        let status = match &self.state {
            State::Idle => None,
            State::Generating { .. } => Some(div().text_size(px(12.0)).text_color(T.accent).child("Claude prépare le workflow…")),
            State::Ready(g) => Some(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_size(px(12.0))
                    .text_color(T.ok)
                    .child(icon(icons::CHECK).size(px(12.0)))
                    .child(format!("{} bloc(s) générés", generate::block_count(&g.workflow.steps))),
            ),
            State::Failed(_) => Some(div().text_size(px(12.0)).text_color(T.err).child("Échec de la génération")),
        };

        let examples = EXAMPLES.iter().enumerate().map(|(i, text)| {
            div()
                .id(("example", i))
                .px(px(10.0))
                .py_2()
                .border_1()
                .border_color(T.border)
                .rounded(px(6.0))
                .text_color(T.text2)
                .cursor_pointer()
                .hover(|d| d.bg(T.hover).text_color(T.text))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.description.update(cx, |d, cx| d.set_value(text.to_string(), window, cx));
                    this.generate(window, cx);
                }))
                .child(*text)
        });

        // Encadré du bas : interprétations, erreur, ou activité de Claude.
        let notice = match &self.state {
            State::Ready(g) if !g.notes.is_empty() => Some((T.warn, g.notes.join("\n"))),
            State::Failed(err) => Some((T.err, err.clone())),
            State::Generating { activity, .. } if !activity.is_empty() => Some((T.text3, activity.iter().rev().take(4).rev().map(|a| format!("✓ {a}")).collect::<Vec<_>>().join("\n"))),
            _ => None,
        };

        let left = div()
            .w(px(440.0))
            .flex_shrink_0()
            .p_5()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .border_r_1()
            .border_color(T.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Décris ton workflow…"))
                    .child(div().p(px(10.0)).border_1().border_color(T.accent).rounded(px(6.0)).bg(T.app).text_size(px(14.0)).line_height(px(22.0)).child(Textarea::new(&self.description).appearance(false).h(px(154.0)))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("generate")
                            .h(px(30.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .bg(T.accent_bg)
                            .text_color(T.accent_on)
                            .font_weight(FontWeight::MEDIUM)
                            .when(generating, |d| d.opacity(0.5))
                            .when(!generating, |d| d.cursor_pointer().hover(|d| d.opacity(0.9)))
                            .on_click(cx.listener(|this, _, window, cx| this.generate(window, cx)))
                            .child(if ready || matches!(self.state, State::Failed(_)) { "Générer à nouveau" } else { "Générer" }),
                    )
                    .child(div().text_size(px(12.0)).text_color(T.text3).child("Ctrl ↵"))
                    .child(div().flex_1())
                    .children(status),
            )
            .child(div().h(px(1.0)).bg(T.border).my_1())
            .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(T.text3).child("EXEMPLES"))
            .child(div().flex().flex_col().gap(px(6.0)).children(examples))
            .child(div().flex_1())
            .when_some(notice, |d, (color, text)| {
                d.child(
                    div()
                        .id("creator-notice")
                        .max_h(px(140.0))
                        .overflow_y_scroll()
                        .flex()
                        .gap_2()
                        .px_3()
                        .py(px(10.0))
                        .rounded(px(6.0))
                        .bg(T.raised)
                        .text_size(px(12.0))
                        .text_color(T.text2)
                        .child(div().flex_shrink_0().text_color(color).child(if color == T.text3 { "›" } else { "!" }))
                        .child(div().flex_1().min_w_0().child(text)),
                )
            });

        let preview = match &self.state {
            State::Ready(generated) if self.show_json => {
                let json = serde_json::to_string_pretty(&generated.workflow).unwrap_or_default();
                let lines: Vec<String> = json.lines().map(str::to_owned).collect();
                let spans = highlight::highlight("workflow.json", &lines);
                div()
                    .font_family(theme::MONO_FONT)
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .children(lines.iter().enumerate().map(|(i, l)| div().whitespace_nowrap().child(code_text(l, spans.get(i).unwrap_or(&Vec::new())))))
                    .into_any_element()
            }
            State::Ready(generated) => {
                let triggers = generated.workflow.triggers();
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(34.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .self_start()
                            .border_1()
                            .border_color(T.border)
                            .rounded(px(17.0))
                            .bg(T.surface)
                            .child(icon(icons::BOLT).size(px(13.0)).text_color(T.accent))
                            .child(div().text_color(T.text2).child("Déclencheur"))
                            .children(triggers.iter().map(|t| div().font_family(theme::MONO_FONT).text_size(px(12.0)).px_1p5().rounded(px(4.0)).bg(T.hover).child(trigger_label(t)))),
                    )
                    .children(generated.workflow.steps.iter().map(|step| div().flex().flex_col().child(connector()).child(step_block(step, false))))
                    .into_any_element()
            }
            State::Generating { .. } => placeholder("Génération en cours…", "Claude lit le projet pour choisir les bonnes commandes.").into_any_element(),
            _ => placeholder("Aperçu", "Décris le workflow à gauche, puis clique sur « Générer ». Les blocs proposés s'affichent ici.").into_any_element(),
        };

        let right = div()
            .flex_1()
            .min_w_0()
            .bg(rgb(0x111215))
            .flex()
            .flex_col()
            .child(
                div()
                    .px_6()
                    .pt_5()
                    .pb(px(14.0))
                    .flex()
                    .items_center()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Aperçu"))
                    .child(div().flex_1())
                    .when(ready, |d| {
                        let on = self.show_json;
                        d.child(
                            div()
                                .id("toggle-json")
                                .h(px(24.0))
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_1()
                                .border_1()
                                .border_color(if on { T.accent_bg } else { T.border })
                                .rounded(px(6.0))
                                .text_size(px(12.0))
                                .text_color(if on { T.text } else { T.text2 })
                                .cursor_pointer()
                                .hover(|d| d.bg(T.hover))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_json = !this.show_json;
                                    cx.notify();
                                }))
                                .child(div().font_family(theme::MONO_FONT).child("{ }"))
                                .child("JSON"),
                        )
                    }),
            )
            .child(div().id("creator-preview").flex_1().min_h_0().overflow_y_scroll().px_6().pb_5().child(preview));

        let footer = div()
            .h(px(56.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_5()
            .border_t_1()
            .border_color(T.border)
            .child(div().text_size(px(12.0)).text_color(T.text2).child("Nom"))
            .child(div().w(px(220.0)).h(px(28.0)).px_2().flex().items_center().border_1().border_color(T.border).rounded(px(6.0)).bg(T.app).child(Input::new(&self.name).appearance(false)))
            .when_some(self.save_error.clone(), |d, err| d.child(div().text_size(px(12.0)).text_color(T.err).child(err)))
            .child(div().flex_1())
            .child(
                div()
                    .id("creator-cancel")
                    .h(px(32.0))
                    .px(px(14.0))
                    .flex()
                    .items_center()
                    .border_1()
                    .border_color(T.border)
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(T.hover))
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx)))
                    .child("Annuler"),
            )
            .child(
                div()
                    .id("creator-edit")
                    .h(px(32.0))
                    .px(px(14.0))
                    .flex()
                    .items_center()
                    .border_1()
                    .border_color(T.border)
                    .rounded(px(6.0))
                    .bg(T.raised)
                    .when(!ready, |d| d.opacity(0.4))
                    .when(ready, |d| d.cursor_pointer().hover(|d| d.bg(T.hover)))
                    .on_click(cx.listener(|this, _, _, cx| this.edit(cx)))
                    .child("Modifier dans l'éditeur"),
            )
            .child(
                div()
                    .id("creator-create")
                    .h(px(32.0))
                    .px_4()
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .bg(T.accent_bg)
                    .text_color(T.accent_on)
                    .font_weight(FontWeight::MEDIUM)
                    .when(!ready, |d| d.opacity(0.4))
                    .when(ready, |d| d.cursor_pointer().hover(|d| d.opacity(0.9)))
                    .on_click(cx.listener(|this, _, _, cx| this.create(cx)))
                    .child("Créer le workflow"),
            );

        div()
            .id("creator")
            .w(px(920.0))
            .h(px(720.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(T.surface)
            .border_1()
            .border_color(T.border_strong)
            .rounded(px(8.0))
            .shadow_lg()
            .text_size(px(13.0))
            .capture_key_down(cx.listener(Self::on_key))
            .child(header)
            .child(div().flex_1().min_h_0().flex().child(left).child(right))
            .child(footer)
    }
}

fn trigger_label(trigger: &Trigger) -> String {
    trigger.label()
}

fn connector() -> Div {
    div().w(px(2.0)).h(px(14.0)).ml(px(24.0)).bg(T.border_strong)
}

fn placeholder(title: &'static str, text: &'static str) -> Div {
    div()
        .mt_16()
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .child(div().font_weight(FontWeight::MEDIUM).child(title))
        .child(div().max_w(px(320.0)).text_center().text_size(px(12.0)).text_color(T.text3).child(text))
}

/// Étiquette de condition : « si échec de lint », « toujours »…
fn condition_tag(condition: &str) -> Option<Div> {
    let (text, fg, bg) = if condition == "always" {
        ("toujours".to_owned(), T.text2, T.hover)
    } else {
        let rest = condition.strip_prefix("steps.")?;
        let (id, status) = rest.rsplit_once('.')?;
        match status {
            "failed" => (format!("si échec de {id}"), T.err, T.err_soft),
            "success" => (format!("si réussite de {id}"), T.ok, T.ok_soft),
            _ => (format!("si {id} ignoré"), T.text2, T.hover),
        }
    };
    Some(div().self_start().text_size(px(11.0)).line_height(px(18.0)).px_1p5().rounded(px(4.0)).bg(bg).text_color(fg).child(text))
}

/// `compact` : carte d'un groupe parallèle, nom et détail sur deux lignes.
fn step_block(step: &Step, compact: bool) -> Div {
    let tag = step.condition.as_deref().and_then(condition_tag);
    let body = if let Some(children) = &step.parallel {
        div()
            .border_1()
            .border_dashed()
            .border_color(T.border_strong)
            .rounded(px(8.0))
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).text_color(T.text3).child(format!("PARALLÈLE · {}", step.name.as_deref().unwrap_or(&step.id).to_uppercase())))
            .child(div().grid().grid_cols(2).gap_2().children(children.iter().map(|child| step_block(child, true))))
    } else {
        let (badge, detail): (Div, Option<String>) = if let Some(command) = &step.run {
            (badge(T.hover).child(div().font_family(theme::MONO_FONT).text_size(px(10.0)).text_color(T.text2).child(">_")), Some(command.clone()))
        } else if step.approve.is_some() {
            (badge(T.selected).child(icon(icons::CHECK).size(px(12.0)).text_color(T.accent)), None)
        } else {
            let model = step.model.as_ref().map(|m| format!(" · {m}")).unwrap_or_default();
            match step.agent.as_deref() {
                Some("codex") => (badge(T.codex_soft).child(icon(icons::CODEX).size(px(12.0)).text_color(T.codex)), Some(format!("Codex{model}"))),
                Some("claude") => (badge(T.claude_soft).child(icon(icons::CLAUDE).size(px(12.0)).text_color(T.claude)), Some(format!("Claude{model}"))),
                _ => (badge(T.selected).child(icon(icons::BOLT).size(px(12.0)).text_color(T.accent)), Some(format!("orchestrateur{model}"))),
            }
        };
        let mono = step.run.is_some();
        let name = step.name.clone().unwrap_or_else(|| step.id.clone());
        let detail_text = |detail: String| div().min_w_0().truncate().text_size(px(12.0)).text_color(if mono { T.text2 } else { T.text3 }).when(mono, |d| d.font_family(theme::MONO_FONT)).child(detail);
        if compact {
            let card = div()
                .px_3()
                .py(px(8.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .border_1()
                .border_color(T.border)
                .rounded(px(6.0))
                .bg(T.raised)
                .min_w_0()
                .child(badge)
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.0)).child(div().truncate().font_weight(FontWeight::MEDIUM).child(name)).children(detail.map(detail_text)));
            return div().flex().flex_col().gap_1().children(tag).child(card);
        }
        div()
            .px_3()
            .py(px(10.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .border_1()
            .border_color(T.border)
            .rounded(px(6.0))
            .bg(T.raised)
            .min_w_0()
            .child(badge)
            .child(div().flex_shrink_0().font_weight(FontWeight::MEDIUM).child(name))
            .children(detail.map(detail_text))
    };
    div().flex().flex_col().gap_1().children(tag).child(body)
}

fn badge(bg: Rgba) -> Div {
    div().size(px(22.0)).flex_shrink_0().rounded(px(4.0)).bg(bg).flex().items_center().justify_center()
}
