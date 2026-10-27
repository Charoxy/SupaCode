//! Éditeur visuel de workflow : bibliothèque de blocs à gauche, flux des étapes au centre,
//! propriétés de l'étape sélectionnée à droite. Tout ce qu'il produit respecte le format
//! des workflows (suite d'étapes, groupes parallèles, conditions), donc s'exécute tel quel.

use std::path::PathBuf;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use supacode_core::edit::{self, StepPath};
use supacode_core::engine::{StepKind, flatten};
use supacode_core::generate::{self, validate};
use supacode_core::workflows::{Step, Trigger, Workflow, save_workflow, write_workflow};

use crate::highlight::{self, code_text};
use crate::icons::{self, icon};
use crate::theme::{self, DARK as T};

pub enum EditorEvent {
    Close,
    /// Enregistré sous cet id ; `run` : lancer le workflow ensuite (« Tester »).
    Saved { id: String, run: bool },
}

impl EventEmitter<EditorEvent> for Editor {}

/// Champs de l'étape sélectionnée.
struct StepFields {
    name: Entity<InputState>,
    id: Entity<InputState>,
    /// Commande, consigne de l'agent ou question de validation.
    body: Option<Entity<TextareaState>>,
    _subscriptions: Vec<Subscription>,
}

/// Champs du workflow lui-même (rien n'est sélectionné).
struct WorkflowFields {
    name: Entity<InputState>,
    description: Entity<InputState>,
    save: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

pub struct Editor {
    project: PathBuf,
    project_name: String,
    /// Fichier d'origine ; `None` pour un nouveau workflow.
    id: Option<String>,
    workflow: Workflow,
    dirty: bool,
    selected: Option<StepPath>,
    step_fields: Option<StepFields>,
    workflow_fields: WorkflowFields,
    show_json: bool,
    save_error: Option<String>,
}

const MODELS: [Option<&str>; 4] = [None, Some("haiku"), Some("sonnet"), Some("opus")];

impl Editor {
    pub fn new(project: PathBuf, project_name: String, id: Option<String>, workflow: Workflow, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workflow_fields = Self::workflow_fields(&workflow, window, cx);
        let dirty = id.is_none();
        Self { project, project_name, id, workflow, dirty, selected: None, step_fields: None, workflow_fields, show_json: false, save_error: None }
    }

    fn workflow_fields(workflow: &Workflow, window: &mut Window, cx: &mut Context<Self>) -> WorkflowFields {
        let glob = workflow
            .triggers()
            .iter()
            .find_map(|t| match t {
                Trigger::Save { save } => Some(save.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "src/**/*".into());
        let name = cx.new(|cx| InputState::new(window, cx).default_value(workflow.name.clone()).placeholder("Nom du workflow"));
        let description = cx.new(|cx| InputState::new(window, cx).default_value(workflow.description.clone().unwrap_or_default()).placeholder("À quoi sert-il ?"));
        let save = cx.new(|cx| InputState::new(window, cx).default_value(glob).placeholder("src/**/*.ts"));
        let subscriptions = [&name, &description, &save]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.apply_workflow_fields(cx);
                    }
                })
            })
            .collect();
        WorkflowFields { name, description, save, _subscriptions: subscriptions }
    }

    fn apply_workflow_fields(&mut self, cx: &mut Context<Self>) {
        let fields = &self.workflow_fields;
        self.workflow.name = fields.name.read(cx).value().to_string();
        let description = fields.description.read(cx).value().trim().to_owned();
        self.workflow.description = (!description.is_empty()).then_some(description);
        let glob = fields.save.read(cx).value().trim().to_owned();
        if let Some(triggers) = &mut self.workflow.on {
            for trigger in triggers.iter_mut() {
                if let Trigger::Save { save } = trigger {
                    *save = glob.clone();
                }
            }
        }
        self.touch(cx);
    }

    fn touch(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.save_error = None;
        cx.notify();
    }

    // — Accès aux étapes

    fn step(&self, path: StepPath) -> Option<&Step> {
        edit::step_at(&self.workflow, path)
    }

    fn step_mut(&mut self, path: StepPath) -> Option<&mut Step> {
        edit::step_at_mut(&mut self.workflow, path)
    }

    // — Sélection et champs

    fn select(&mut self, path: Option<StepPath>, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = path;
        self.step_fields = path.and_then(|p| self.step(p).cloned()).map(|step| self.build_fields(&step, window, cx));
        cx.notify();
    }

    fn build_fields(&self, step: &Step, window: &mut Window, cx: &mut Context<Self>) -> StepFields {
        let name = cx.new(|cx| InputState::new(window, cx).default_value(step.label().to_owned()));
        let id = cx.new(|cx| InputState::new(window, cx).default_value(step.id.clone()));
        let body_text = match step.kind() {
            StepKind::Run => step.run.clone(),
            StepKind::Agent => step.prompt.clone(),
            StepKind::Approve => step.approve.clone(),
            StepKind::Parallel => None,
        };
        let rows = if step.kind() == StepKind::Agent { 8 } else { 5 };
        let placeholder = match step.kind() {
            StepKind::Run => "npm test",
            StepKind::Agent => "Ce que l'agent doit faire. Ex. : Corrige ces erreurs :\n{{steps.tests.output}}",
            _ => "Valider le commit ?",
        };
        let body = body_text.map(|text| cx.new(|cx| TextareaState::new(window, cx).rows(rows).default_value(text).placeholder(placeholder)));
        let mut subscriptions = vec![
            cx.subscribe(&name, |this: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.apply_name(cx);
                }
            }),
            cx.subscribe(&id, |this: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.apply_id(cx);
                }
            }),
        ];
        if let Some(body) = &body {
            subscriptions.push(cx.subscribe(body, |this: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.apply_body(cx);
                }
            }));
        }
        StepFields { name, id, body, _subscriptions: subscriptions }
    }

    fn apply_name(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(fields)) = (self.selected, &self.step_fields) else { return };
        let name = fields.name.read(cx).value().trim().to_owned();
        if let Some(step) = self.step_mut(path) {
            step.name = (!name.is_empty()).then_some(name);
        }
        self.touch(cx);
    }

    fn apply_body(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(fields)) = (self.selected, &self.step_fields) else { return };
        let Some(body) = &fields.body else { return };
        let text = body.read(cx).value().to_string();
        if let Some(step) = self.step_mut(path) {
            match step.kind() {
                StepKind::Run => step.run = Some(text),
                StepKind::Agent => step.prompt = Some(text),
                StepKind::Approve => step.approve = Some(text),
                StepKind::Parallel => {}
            }
        }
        self.touch(cx);
    }

    /// Renomme l'étape (les conditions et `{{steps.<id>…}}` qui la citent suivent).
    fn apply_id(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(fields)) = (self.selected, &self.step_fields) else { return };
        let new = fields.id.read(cx).value().trim().to_owned();
        if edit::rename_step(&mut self.workflow, path, &new) {
            self.touch(cx);
        }
    }

    // — Modifications de structure

    fn add(&mut self, kind: StepKind, window: &mut Window, cx: &mut Context<Self>) {
        let path = edit::add_step(&mut self.workflow, self.selected, kind);
        self.select(Some(path), window, cx);
        self.touch(cx);
    }

    fn move_selected(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.selected.and_then(|p| edit::move_step(&mut self.workflow, p, delta)) else { return };
        self.select(Some(path), window, cx);
        self.touch(cx);
    }

    fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.selected else { return };
        edit::remove_step(&mut self.workflow, path);
        self.select(None, window, cx);
        self.touch(cx);
    }

    fn set_agent(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(path) = self.selected else { return };
        if let Some(step) = self.step_mut(path) {
            step.agent = Some(agent.to_owned());
            if agent == "codex" {
                step.model = None;
            }
        }
        self.touch(cx);
    }

    fn set_model(&mut self, model: Option<&str>, cx: &mut Context<Self>) {
        let Some(path) = self.selected else { return };
        if let Some(step) = self.step_mut(path) {
            step.model = model.map(str::to_owned);
        }
        self.touch(cx);
    }

    fn set_condition(&mut self, condition: Option<String>, cx: &mut Context<Self>) {
        let Some(path) = self.selected else { return };
        if let Some(step) = self.step_mut(path) {
            step.condition = condition;
        }
        self.touch(cx);
    }

    fn toggle_trigger(&mut self, name: &'static str, cx: &mut Context<Self>) {
        let glob = self.workflow_fields.save.read(cx).value().trim().to_owned();
        edit::toggle_trigger(&mut self.workflow, name, &glob);
        self.touch(cx);
    }

    // — Enregistrement

    fn save(&mut self, run: bool, cx: &mut Context<Self>) {
        if let Err(err) = validate(&self.workflow) {
            self.save_error = Some(format!("{err:#}"));
            cx.notify();
            return;
        }
        let result = match &self.id {
            Some(id) => write_workflow(&self.project, id, &self.workflow).map(|()| id.clone()),
            None => save_workflow(&self.project, &generate::slug(&self.workflow.name), &self.workflow),
        };
        match result {
            Ok(id) => {
                self.id = Some(id.clone());
                self.dirty = false;
                cx.emit(EditorEvent::Saved { id, run });
            }
            Err(err) => self.save_error = Some(format!("{err:#}")),
        }
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dirty {
            cx.emit(EditorEvent::Close);
            return;
        }
        let answer = window.prompt(PromptLevel::Warning, "Quitter l'éditeur sans enregistrer ?", Some("Les modifications de ce workflow seront perdues."), &["Quitter", "Annuler"], cx);
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |_, cx| cx.emit(EditorEvent::Close));
            }
        })
        .detach();
    }
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let validity = validate(&self.workflow).map_err(|e| format!("{e:#}"));
        div()
            .id("editor")
            .size_full()
            .flex()
            .flex_col()
            .bg(T.surface)
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let k = &event.keystroke;
                if k.modifiers.control && k.key == "s" {
                    this.save(false, cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().flex().child(self.render_library(cx)).child(self.render_canvas(&validity, cx)).child(self.render_properties(cx)))
    }
}

// — Rendu

impl Editor {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let file = self.id.as_ref().map(|id| format!(".supacode/workflows/{id}.json")).unwrap_or_else(|| "nouveau workflow".into());
        div()
            .h(px(48.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .border_b_1()
            .border_color(T.border)
            .bg(T.panel)
            .child(
                div()
                    .id("editor-back")
                    .size(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .text_color(T.text2)
                    .cursor_pointer()
                    .hover(|d| d.bg(T.hover))
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
                    .child(icon(icons::CHEVRON_LEFT).size(px(14.0))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(div().text_color(T.text3).child(self.project_name.clone()))
                    .child(div().text_color(T.text3).child("›"))
                    .child(div().text_color(T.text3).child("Workflows"))
                    .child(div().text_color(T.text3).child("›"))
                    .child(div().font_weight(FontWeight::MEDIUM).child(if self.workflow.name.is_empty() { "Sans nom".to_owned() } else { self.workflow.name.clone() })),
            )
            .children(self.workflow.triggers().iter().map(|t| div().font_family(theme::MONO_FONT).text_size(px(11.0)).px_1p5().rounded(px(4.0)).border_1().border_color(T.border).text_color(T.text2).child(t.label())))
            .child(div().text_size(px(12.0)).text_color(if self.dirty { T.warn } else { T.text3 }).child(if self.dirty { "Modifications non enregistrées".to_owned() } else { file }))
            .child(div().flex_1())
            .when_some(self.save_error.clone(), |d, err| d.child(div().max_w(px(320.0)).truncate().text_size(px(12.0)).text_color(T.err).child(err)))
            .child(
                header_button("editor-json", self.show_json)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_json = !this.show_json;
                        cx.notify();
                    }))
                    .child(div().font_family(theme::MONO_FONT).child("{ }"))
                    .child(if self.show_json { "Voir les blocs" } else { "Voir le JSON" }),
            )
            .child(
                header_button("editor-test", false)
                    .on_click(cx.listener(|this, _, _, cx| this.save(true, cx)))
                    .child(icon(icons::PLAY).size(px(10.0)))
                    .child("Tester"),
            )
            .child(
                div()
                    .id("editor-save")
                    .h(px(28.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .bg(T.accent_bg)
                    .text_color(T.accent_on)
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .hover(|d| d.opacity(0.9))
                    .on_click(cx.listener(|this, _, _, cx| this.save(false, cx)))
                    .child("Enregistrer"),
            )
    }

    fn render_library(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let block = |id: &'static str, kind: StepKind, badge: Div, title: &'static str, text: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(10.0))
                .p_2()
                .border_1()
                .border_color(T.border)
                .rounded(px(6.0))
                .bg(T.raised)
                .cursor_pointer()
                .hover(|d| d.border_color(T.border_strong).bg(T.hover))
                .on_click(cx.listener(move |this, _, window, cx| this.add(kind, window, cx)))
                .child(badge)
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.0)).child(div().font_weight(FontWeight::MEDIUM).child(title)).child(div().text_size(px(11.0)).text_color(T.text3).child(text)))
        };
        let triggers = self.workflow.triggers();
        let has = |name: &str| {
            triggers.iter().any(|t| match t {
                Trigger::Save { .. } => name == "save",
                Trigger::Named(n) => n == name,
            })
        };
        let trigger_chips = [("manual", "manuel"), ("commit", "commit"), ("push", "push"), ("save", "save")].map(|(name, label)| {
            chip(format!("trigger-{name}"), label, has(name))
                .font_family(theme::MONO_FONT)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_trigger(name, cx)))
        });
        div()
            .w(px(232.0))
            .flex_shrink_0()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .border_r_1()
            .border_color(T.border)
            .bg(T.panel)
            .child(section("Blocs"))
            .child(block("lib-run", StepKind::Run, kind_badge(StepKind::Run, None), "Commande", "Lance une commande shell"))
            .child(block("lib-agent", StepKind::Agent, kind_badge(StepKind::Agent, None), "Agent", "Confie une tâche à Claude ou Codex"))
            .child(block("lib-approve", StepKind::Approve, kind_badge(StepKind::Approve, None), "Validation", "Attend ta décision"))
            .child(block("lib-parallel", StepKind::Parallel, kind_badge(StepKind::Parallel, None), "Parallèle", "Plusieurs branches en même temps"))
            .child(section("Déclencheurs").mt_3())
            .child(div().flex().flex_wrap().gap_1p5().children(trigger_chips))
            .child(div().flex_1())
            .child(div().text_size(px(12.0)).text_color(T.text3).child("Clique sur un bloc pour l'ajouter après l'étape sélectionnée (ou dans le groupe parallèle sélectionné)."))
    }

    fn render_canvas(&self, validity: &Result<(), String>, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if self.show_json {
            let json = serde_json::to_string_pretty(&self.workflow).unwrap_or_default();
            let lines: Vec<String> = json.lines().map(str::to_owned).collect();
            let spans = highlight::highlight("workflow.json", &lines);
            div()
                .p_6()
                .font_family(theme::MONO_FONT)
                .text_size(px(12.0))
                .line_height(px(18.0))
                .children(lines.iter().enumerate().map(|(i, l)| div().whitespace_nowrap().child(code_text(l, spans.get(i).unwrap_or(&Vec::new())))))
                .into_any_element()
        } else {
            let mut flow = div().w(px(560.0)).py_8().flex().flex_col().items_stretch();
            flow = flow.child(
                div()
                    .id("canvas-trigger")
                    .h(px(34.0))
                    .px_3()
                    .self_start()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .border_1()
                    .border_color(if self.selected.is_none() { T.accent_bg } else { T.border })
                    .rounded(px(17.0))
                    .bg(T.surface)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| this.select(None, window, cx)))
                    .child(icon(icons::BOLT).size(px(13.0)).text_color(T.accent))
                    .child(div().text_color(T.text2).child("Déclencheur"))
                    .children(self.workflow.triggers().iter().map(|t| div().font_family(theme::MONO_FONT).text_size(px(12.0)).px_1p5().rounded(px(4.0)).bg(T.hover).child(t.label()))),
            );
            for (i, step) in self.workflow.steps.iter().enumerate() {
                flow = flow.child(connector()).child(self.render_step(step, (i, None), cx));
            }
            if self.workflow.steps.is_empty() {
                flow = flow.child(connector()).child(div().p_4().border_1().border_dashed().border_color(T.border_strong).rounded(px(6.0)).text_color(T.text3).text_center().child("Ajoute un premier bloc depuis la bibliothèque à gauche."));
            }
            div().flex().justify_center().child(flow).into_any_element()
        };
        let count = flatten(&self.workflow.steps).len();
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(rgb(0x111215))
            .child(div().id("canvas").flex_1().min_h_0().overflow_scroll().child(content))
            .child(
                div()
                    .h(px(30.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .border_t_1()
                    .border_color(T.border)
                    .text_size(px(12.0))
                    .child(div().text_color(T.text3).child(format!("{count} bloc(s)")))
                    .child(match validity {
                        Ok(()) => div().flex().items_center().gap_1().text_color(T.ok).child(icon(icons::CHECK).size(px(12.0))).child("Workflow valide"),
                        Err(err) => div().min_w_0().truncate().text_color(T.err).child(err.clone()),
                    }),
            )
    }

    fn render_step(&self, step: &Step, path: StepPath, cx: &mut Context<Self>) -> Div {
        let selected = self.selected == Some(path);
        let tag = step.condition.as_deref().and_then(condition_tag);
        let id = SharedString::from(format!("canvas-{}-{}", path.0, path.1.map_or(-1, |j| j as i64)));
        let body = if let Some(children) = &step.parallel {
            let group_path = path;
            div()
                .border_1()
                .border_dashed()
                .border_color(if selected { T.accent_bg } else { T.border_strong })
                .rounded(px(8.0))
                .p(px(10.0))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .id(id)
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .cursor_pointer()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if selected { T.accent } else { T.text3 })
                        .on_click(cx.listener(move |this, _, window, cx| this.select(Some(group_path), window, cx)))
                        .child(icon(icons::WORKFLOW).size(px(11.0)))
                        .child(format!("PARALLÈLE · {}", step.label().to_uppercase())),
                )
                .child(div().grid().grid_cols(2).gap_2().children(children.iter().enumerate().map(|(j, child)| self.render_step(child, (path.0, Some(j)), cx))))
                .child(div().text_size(px(11.0)).text_color(T.text3).child("Toutes les branches doivent réussir"))
                .into_any_element()
        } else {
            let (kind_label, detail) = match step.kind() {
                StepKind::Run => ("COMMANDE", step.run.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "commande à définir".into())),
                StepKind::Agent => {
                    let agent = match step.agent.as_deref() {
                        Some("claude") => "Claude",
                        Some("codex") => "Codex",
                        _ => "orchestrateur",
                    };
                    ("AGENT", format!("{agent}{}", step.model.as_ref().map(|m| format!(" · {m}")).unwrap_or_default()))
                }
                StepKind::Approve => ("VALIDATION", step.approve.clone().unwrap_or_default()),
                StepKind::Parallel => ("PARALLÈLE", String::new()),
            };
            let mono = step.kind() == StepKind::Run;
            div()
                .id(id)
                .px_3()
                .py(px(10.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .border_1()
                .border_color(if selected { T.accent_bg } else { T.border })
                .when(selected, |d| d.bg(T.selected))
                .when(!selected, |d| d.bg(T.raised).hover(|d| d.border_color(T.border_strong)))
                .rounded(px(6.0))
                .min_w_0()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| this.select(Some(path), window, cx)))
                .child(kind_badge(step.kind(), step.agent.as_deref()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(div().flex().gap_2().items_center().child(div().text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).text_color(T.text3).child(kind_label)).child(div().truncate().font_weight(FontWeight::MEDIUM).child(step.label().to_owned())))
                        .child(div().truncate().text_size(px(12.0)).text_color(T.text2).when(mono, |d| d.font_family(theme::MONO_FONT)).child(detail)),
                )
                .into_any_element()
        };
        div().flex().flex_col().gap_1().min_w_0().children(tag).child(body)
    }

    fn render_properties(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().id("properties").flex_1().min_h_0().overflow_y_scroll().p_4().flex().flex_col().gap(px(14.0));
        let mut kind_name = "Workflow";
        match (self.selected.and_then(|p| self.step(p).map(|s| (p, s))), &self.step_fields) {
            (Some((path, step)), Some(fields)) => {
                kind_name = match step.kind() {
                    StepKind::Run => "Commande",
                    StepKind::Agent => "Agent",
                    StepKind::Approve => "Validation",
                    StepKind::Parallel => "Parallèle",
                };
                body = body.child(field("Nom", input_box(Input::new(&fields.name).appearance(false))));
                body = body.child(field("Identifiant", input_box(Input::new(&fields.id).appearance(false))).child(hint("Utilisé dans les conditions et {{steps.<id>.output}}.")));
                body = body.child(self.render_condition(step, path, cx));
                match step.kind() {
                    StepKind::Agent => {
                        let agent = step.agent.as_deref().unwrap_or("auto");
                        let agents = [("auto", "Auto"), ("claude", "Claude"), ("codex", "Codex")].map(|(value, label)| {
                            chip(format!("agent-{value}"), label, agent == value).on_click(cx.listener(move |this, _, _, cx| this.set_agent(value, cx)))
                        });
                        body = body.child(field("Agent", div().flex().gap_1p5().children(agents)).child(hint("Auto : l'orchestrateur choisit selon la demande.")));
                        if agent != "codex" {
                            let models = MODELS.map(|model| {
                                chip(format!("model-{}", model.unwrap_or("defaut")), model.unwrap_or("défaut"), step.model.as_deref() == model)
                                    .on_click(cx.listener(move |this, _, _, cx| this.set_model(model, cx)))
                            });
                            body = body.child(field("Modèle", div().flex().gap_1p5().children(models)));
                        }
                    }
                    StepKind::Parallel => {
                        body = body.child(hint("Les branches s'exécutent en même temps ; le groupe échoue si l'une d'elles échoue. Sélectionne le groupe puis un bloc de la bibliothèque pour ajouter une branche."));
                    }
                    _ => {}
                }
                if let Some(text) = &fields.body {
                    let label = match step.kind() {
                        StepKind::Run => "Commande",
                        StepKind::Agent => "Instructions",
                        _ => "Question",
                    };
                    let mono = step.kind() == StepKind::Run;
                    body = body.child(
                        field(label, div().p_2().border_1().border_color(T.border).rounded(px(6.0)).bg(T.app).when(mono, |d| d.font_family(theme::MONO_FONT).text_size(px(12.0))).child(Textarea::new(text).appearance(false)))
                            .when(step.kind() != StepKind::Run, |d| d.child(hint("Variables : {{git.diff}}, {{git.branch}}, {{steps.<id>.output}}, {{file}}."))),
                    );
                }
            }
            _ => {
                let fields = &self.workflow_fields;
                body = body.child(field("Nom", input_box(Input::new(&fields.name).appearance(false))));
                body = body.child(field("Description", input_box(Input::new(&fields.description).appearance(false))));
                let has_save = self.workflow.triggers().iter().any(|t| matches!(t, Trigger::Save { .. }));
                if has_save {
                    body = body.child(field("Fichiers surveillés (save)", input_box(Input::new(&fields.save).appearance(false)).font_family(theme::MONO_FONT)).child(hint("Motif glob relatif au projet, ex. src/**/*.ts.")));
                }
                body = body.child(hint("Sélectionne un bloc du canevas pour modifier ses propriétés. Les déclencheurs se choisissent à gauche."));
            }
        }

        let actions = self.selected.map(|_| {
            let button = |id: &'static str, label: &'static str, danger: bool| {
                div()
                    .id(id)
                    .h(px(28.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(if danger { T.err_soft } else { T.border })
                    .rounded(px(6.0))
                    .text_size(px(12.0))
                    .text_color(if danger { T.err } else { T.text })
                    .cursor_pointer()
                    .hover(|d| d.bg(T.hover))
                    .child(label)
            };
            div()
                .flex_shrink_0()
                .p_3()
                .flex()
                .gap_2()
                .border_t_1()
                .border_color(T.border)
                .child(button("prop-up", "Monter", false).on_click(cx.listener(|this, _, window, cx| this.move_selected(-1, window, cx))))
                .child(button("prop-down", "Descendre", false).on_click(cx.listener(|this, _, window, cx| this.move_selected(1, window, cx))))
                .child(div().flex_1())
                .child(button("prop-delete", "Supprimer le bloc", true).on_click(cx.listener(|this, _, window, cx| this.delete_selected(window, cx))))
        });

        div()
            .w(px(320.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(T.border)
            .bg(T.panel)
            .child(
                div()
                    .h(px(40.0))
                    .flex_shrink_0()
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(T.border)
                    .child(div().font_weight(FontWeight::MEDIUM).child("Propriétés"))
                    .child(div().text_size(px(11.0)).px_1p5().rounded(px(4.0)).bg(T.hover).text_color(T.text2).child(kind_name)),
            )
            .child(body)
            .children(actions)
    }

    /// Condition d'exécution : par défaut, toujours, ou selon le résultat d'une étape précédente.
    fn render_condition(&self, step: &Step, path: StepPath, cx: &mut Context<Self>) -> Div {
        let current = step.condition.clone();
        let (mode, target) = match current.as_deref() {
            None => ("default", None),
            Some("always") => ("always", None),
            Some(c) => {
                let rest = c.strip_prefix("steps.").unwrap_or(c);
                match rest.rsplit_once('.') {
                    Some((id, "failed")) => ("failed", Some(id.to_owned())),
                    Some((id, _)) => ("success", Some(id.to_owned())),
                    None => ("default", None),
                }
            }
        };
        let previous = edit::previous_ids(&self.workflow, path);
        let first = previous.last().cloned();
        let modes = [("default", "Par défaut"), ("always", "Toujours"), ("failed", "Si échec"), ("success", "Si réussite")].map(|(value, label)| {
            let enabled = matches!(value, "default" | "always") || first.is_some();
            let (first, target) = (first.clone(), target.clone());
            chip(format!("cond-{value}"), label, mode == value).when(!enabled, |d| d.opacity(0.4)).on_click(cx.listener(move |this, _, _, cx| {
                let id = target.clone().or(first.clone());
                let condition = match (value, id) {
                    ("always", _) => Some("always".to_owned()),
                    ("failed", Some(id)) => Some(format!("steps.{id}.failed")),
                    ("success", Some(id)) => Some(format!("steps.{id}.success")),
                    ("default", _) => None,
                    _ => return,
                };
                this.set_condition(condition, cx);
            }))
        });
        let mut out = field("Condition", div().flex().flex_wrap().gap_1p5().children(modes));
        if matches!(mode, "failed" | "success") {
            let status = if mode == "failed" { "failed" } else { "success" };
            let targets = previous.iter().map(|id| {
                let id = id.clone();
                let on = target.as_deref() == Some(id.as_str());
                chip(format!("cond-target-{id}"), &id, on).font_family(theme::MONO_FONT).on_click(cx.listener(move |this, _, _, cx| this.set_condition(Some(format!("steps.{id}.{status}")), cx)))
            });
            out = out.child(div().text_size(px(11.0)).text_color(T.text3).child("de l'étape :")).child(div().flex().flex_wrap().gap_1p5().children(targets));
        } else {
            out = out.child(hint(if mode == "always" {
                "S'exécute même si une étape précédente a échoué."
            } else {
                "S'exécute si aucune étape précédente n'a échoué."
            }));
        }
        out
    }
}

fn header_button(id: &'static str, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.0))
        .px_2()
        .flex()
        .items_center()
        .gap_1p5()
        .border_1()
        .border_color(if on { T.accent_bg } else { T.border })
        .rounded(px(6.0))
        .text_size(px(12.0))
        .text_color(if on { T.text } else { T.text2 })
        .cursor_pointer()
        .hover(|d| d.bg(T.hover))
}

fn chip(id: String, label: &str, on: bool) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .h(px(24.0))
        .px_2()
        .flex()
        .items_center()
        .rounded(px(6.0))
        .border_1()
        .border_color(if on { T.accent_bg } else { T.border })
        .when(on, |d| d.bg(T.selected))
        .text_size(px(12.0))
        .text_color(if on { T.text } else { T.text2 })
        .cursor_pointer()
        .hover(|d| d.bg(T.hover))
        .child(label.to_owned())
}

fn section(text: &str) -> Div {
    div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(T.text3).child(text.to_uppercase())
}

fn field(label: &str, control: impl IntoElement) -> Div {
    div().flex().flex_col().gap_1p5().child(div().text_size(px(12.0)).text_color(T.text2).child(label.to_owned())).child(control)
}

fn hint(text: &str) -> Div {
    div().text_size(px(11.0)).text_color(T.text3).child(text.to_owned())
}

fn input_box(input: Input) -> Div {
    div().h(px(28.0)).px_2().flex().items_center().border_1().border_color(T.border).rounded(px(6.0)).bg(T.app).child(input)
}

fn connector() -> Div {
    div().w(px(2.0)).h(px(16.0)).ml(px(24.0)).bg(T.border_strong)
}

fn kind_badge(kind: StepKind, agent: Option<&str>) -> Div {
    let base = div().size(px(22.0)).flex_shrink_0().rounded(px(4.0)).flex().items_center().justify_center();
    match kind {
        StepKind::Run => base.bg(T.hover).child(div().font_family(theme::MONO_FONT).text_size(px(10.0)).text_color(T.text2).child(">_")),
        StepKind::Approve => base.bg(T.selected).child(icon(icons::CHECK).size(px(12.0)).text_color(T.accent)),
        StepKind::Parallel => base.bg(T.hover).child(icon(icons::WORKFLOW).size(px(12.0)).text_color(T.text2)),
        StepKind::Agent => match agent {
            Some("codex") => base.bg(T.codex_soft).child(icon(icons::CODEX).size(px(12.0)).text_color(T.codex)),
            Some("claude") => base.bg(T.claude_soft).child(icon(icons::CLAUDE).size(px(12.0)).text_color(T.claude)),
            _ => base.bg(T.selected).child(icon(icons::BOLT).size(px(12.0)).text_color(T.accent)),
        },
    }
}

/// Étiquette de condition au-dessus d'un bloc.
fn condition_tag(condition: &str) -> Option<Div> {
    let (text, fg, bg) = if condition == "always" {
        ("toujours".to_owned(), T.text2, T.hover)
    } else {
        let (id, status) = condition.strip_prefix("steps.")?.rsplit_once('.')?;
        match status {
            "failed" => (format!("si échec de {id}"), T.err, T.err_soft),
            "success" => (format!("si réussite de {id}"), T.ok, T.ok_soft),
            _ => (format!("si {id} ignoré"), T.text2, T.hover),
        }
    };
    Some(div().self_start().text_size(px(11.0)).line_height(px(18.0)).px_1p5().rounded(px(4.0)).bg(bg).text_color(fg).child(text))
}
