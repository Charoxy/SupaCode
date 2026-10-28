//! Onglet Prompts : la bibliothèque de prompts du projet (`.supacode/prompts/`), versionnée
//! par git. Lancement dans une conversation, historique, comparaison entre versions,
//! et « Enregistrer ce prompt » depuis une conversation.

use std::collections::HashMap;

use similar::{ChangeTag, TextDiff};
use supacode_core::audit::{self, kind};
use supacode_core::engine;
use supacode_core::prompts::{self, PromptFile, Version};

use crate::ask::{AskDialog, AskEvent};

use super::*;

/// Ce que le panneau de droite montre d'un prompt.
pub(super) struct PromptView {
    pub id: String,
    pub version: String,
    pub history: Vec<Version>,
    /// Version comparée à l'actuelle, et ses lignes (−, + ou identiques).
    pub compare: Option<(String, Vec<(ChangeTag, String)>)>,
}

/// Lancement en attente d'une saisie.
pub(super) enum Pending {
    Prompt { root: PathBuf, file: PromptFile },
    Workflow { root: PathBuf, file: WorkflowFile },
}

impl Workspace {
    pub(super) fn load_prompts(&mut self) {
        self.prompts = match self.workflows_project() {
            Some(project) => prompts::load_all(&project.path).map_err(|e| format!("{e:#}")),
            None => Ok(Vec::new()),
        };
    }

    pub(super) fn open_prompt(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(project) = self.workflows_project() else { return };
        self.right = RightPanel::Prompt;
        self.prompt_view = Some(PromptView { id: id.clone(), version: String::new(), history: Vec::new(), compare: None });
        cx.notify();
        let root = project.path;
        let task = cx.spawn(async move |this, cx| {
            let (version, history) = cx.background_executor().spawn({
                let id = id.clone();
                async move { (prompts::version(&root, &id), prompts::history(&root, &id)) }
            }).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(view) = this.prompt_view.as_mut().filter(|v| v.id == id) {
                    view.version = version;
                    view.history = history;
                    cx.notify();
                }
            });
        });
        self._tasks.push(task);
    }

    fn compare_prompt(&mut self, commit: String, cx: &mut Context<Self>) {
        let (Some(project), Some(view)) = (self.workflows_project(), self.prompt_view.as_mut()) else { return };
        if view.compare.as_ref().is_some_and(|(c, _)| *c == commit) {
            view.compare = None;
            cx.notify();
            return;
        }
        let current = prompts::load(&project.path, &view.id).map(|p| p.body).unwrap_or_default();
        let old = prompts::at_version(&project.path, &view.id, &commit).unwrap_or_default();
        let lines = TextDiff::from_lines(&old, &current).iter_all_changes().map(|c| (c.tag(), c.value().trim_end_matches('\n').to_owned())).collect();
        view.compare = Some((commit, lines));
        cx.notify();
    }

    pub(super) fn new_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.workflows_project() else { return };
        match prompts::save(&project.path, "Nouveau prompt", Some("À quoi sert ce prompt, en une phrase."), prompts::template()) {
            Ok(id) => {
                self.load_prompts();
                self.open_prompt(id.clone(), cx);
                self.edit_prompt(&id, cx);
            }
            Err(e) => self.toast(format!("Impossible de créer le prompt : {e:#}"), cx),
        }
    }

    fn edit_prompt(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(project) = self.workflows_project() else { return };
        let path = prompts::dir(&project.path).join(format!("{id}.md"));
        if std::process::Command::new("cmd").args(["/c", "start", ""]).arg(&path).spawn().is_err() {
            self.open_file(&project.path, prompts::relative_path(id), cx);
        }
    }

    /// Lance un prompt de la bibliothèque, après avoir demandé ses variables s'il en a.
    fn launch_prompt(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.workflows_project() else { return };
        let file = match prompts::load(&project.path, id) {
            Ok(file) => file,
            Err(e) => {
                self.toast(format!("{e:#}"), cx);
                return;
            }
        };
        if file.variables.is_empty() {
            self.run_prompt(project.path, file, HashMap::new(), window, cx);
            return;
        }
        // Valeurs proposées : le fichier ouvert pour {{file}} / {{fichier}}.
        let opened = self.opened.as_ref().map(|o| o.relative.clone()).unwrap_or_default();
        let fields = file
            .variables
            .iter()
            .map(|name| {
                let value = if matches!(name.as_str(), "file" | "fichier") { opened.clone() } else { String::new() };
                (name.clone(), name.replace(['_', '-'], " "), value)
            })
            .collect();
        let subtitle = file.description.clone();
        let title = format!("Lancer « {} »", file.title);
        self.ask_user(title, subtitle, "Lancer", fields, Pending::Prompt { root: project.path, file }, window, cx);
    }

    fn run_prompt(&mut self, root: PathBuf, file: PromptFile, values: HashMap<String, String>, window: &mut Window, cx: &mut Context<Self>) {
        let text = engine::render(&file.body, &values);
        let version = prompts::version(&root, &file.id);
        audit::record(&root, kind::PROMPT, format!("Prompt « {} » lancé (version {version})", file.title), serde_json::json!({ "prompt": file.id, "version": version, "variables": values }), None);
        let Some(project) = self.projects.iter().find(|p| same_path(&p.path, &root)).cloned() else { return };
        match self.active_tab().filter(|t| same_path(&t.project.path, &root) && !t.terminal.read(cx).exited) {
            Some(tab) => {
                let terminal = tab.terminal.clone();
                terminal.read(cx).paste(&text);
                let task = cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(Duration::from_millis(150)).await;
                    let _ = terminal.update(cx, |t, _| t.write(b"\r".as_slice()));
                });
                self._tasks.push(task);
                self.refocus(window, cx);
            }
            None => self.open_conversation(project, None, Some(text), window, cx),
        }
    }

    /// Ouvre la fenêtre de saisie ; `pending` part à la validation.
    pub(super) fn ask_user(&mut self, title: String, subtitle: Option<String>, submit: &'static str, fields: Vec<(String, String, String)>, pending: Pending, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| AskDialog::new(title, subtitle, submit, fields, window, cx));
        cx.subscribe_in(&dialog, window, |this, _, event: &AskEvent, window, cx| {
            let Some((_, pending)) = this.ask.take() else { return };
            if let AskEvent::Submit(values) = event {
                match pending {
                    Pending::Prompt { root, file } => this.run_prompt(root, file, values.clone(), window, cx),
                    Pending::Workflow { root, file } => this.start_workflow(root, file, "manuel".into(), values.clone(), true, cx),
                }
            }
            this.refocus(window, cx);
            cx.notify();
        })
        .detach();
        self.ask = Some((dialog, pending));
        cx.notify();
    }

    /// Lancement manuel d'un workflow : sa question d'abord, s'il en pose une.
    pub(super) fn launch_workflow(&mut self, root: PathBuf, file: WorkflowFile, window: &mut Window, cx: &mut Context<Self>) {
        match file.workflow.ask.clone() {
            Some(question) => {
                let title = format!("Lancer « {} »", file.workflow.name);
                let subtitle = file.workflow.description.clone();
                self.ask_user(title, subtitle, "Lancer", vec![("saisie".into(), question, String::new())], Pending::Workflow { root, file }, window, cx);
            }
            None => self.run_workflow(root, file, cx),
        }
    }

    /// Garde la dernière demande de la conversation ouverte dans la bibliothèque.
    pub(super) fn save_conversation_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else { return };
        let (project, session) = (tab.project.clone(), tab.conversation_id.clone());
        let transcript = claude::projects_dir().filter(|_| !project.id.is_empty()).map(|d| d.join(&project.id).join(format!("{session}.jsonl")));
        let Some(text) = transcript.and_then(|t| prompts::last_user_message(&t)) else {
            self.toast("Rien à enregistrer : envoie d'abord une demande dans cette conversation.", cx);
            return;
        };
        let title: String = text.lines().next().unwrap_or("Prompt").chars().take(60).collect();
        match prompts::save(&project.path, &title, None, &text) {
            Ok(id) => {
                audit::record(&project.path, kind::PROMPT, format!("Prompt « {title} » enregistré depuis une conversation"), serde_json::json!({ "prompt": id, "conversation": session }), None);
                self.workflow_project = None;
                self.load_prompts();
                self.sidebar_tab = SidebarTab::Prompts;
                self.sidebar_open = true;
                self.open_prompt(id.clone(), cx);
                self.toast(format!("Prompt enregistré : {} (à versionner avec git)", prompts::relative_path(&id)), cx);
            }
            Err(e) => self.toast(format!("Impossible d'enregistrer le prompt : {e:#}"), cx),
        }
    }

    /// Ajoute un workflow prêt à l'emploi au projet (sans jamais remplacer un fichier).
    fn add_template(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.workflows_project() else { return };
        let workflow = supacode_core::templates::test_first(&project.path);
        match supacode_core::workflows::save_workflow(&project.path, "correctif-tests-d-abord", &workflow) {
            Ok(id) => {
                self.load_workflows(cx);
                self.toast(format!("Workflow ajouté : .supacode/workflows/{id}.json (commande de test : {})", supacode_core::templates::test_command(&project.path)), cx);
            }
            Err(e) => self.toast(format!("Impossible d'ajouter le modèle : {e:#}"), cx),
        }
    }

    // — Rendu

    pub(super) fn render_templates(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_shrink_0()
            .border_t_1()
            .border_color(T.border)
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_label("Modèles").px_1p5())
            .child(
                div()
                    .id("template-test-first")
                    .px_1p5()
                    .py_1p5()
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|d| d.bg(T.hover))
                    .on_click(cx.listener(|this, _, _, cx| this.add_template(cx)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(div().truncate().child("Correctif : tests d'abord"))
                            .child(div().text_size(px(11.0)).text_color(T.text3).child("Le test reproduit le bug et doit échouer, puis passer après le correctif.")),
                    )
                    .child(icon(icons::PLUS).size(px(13.0)).text_color(T.text3)),
            )
    }

    pub(super) fn render_prompts(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().id("prompt-list").flex_1().min_h_0().overflow_y_scroll().px_2().pt_1().flex().flex_col().gap(px(2.0));
        match &self.prompts {
            Err(error) => list = list.child(div().px_1p5().py_1().text_size(px(12.0)).text_color(T.err).child(format!("✖ {error}"))),
            Ok(_) if self.workflows_project().is_none() => {
                list = list.child(div().px_1p5().py_1().text_size(px(12.0)).text_color(T.text3).child("Ajoute un projet pour voir ses prompts."));
            }
            Ok(prompts) if prompts.is_empty() => {
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
                        .child(div().font_weight(FontWeight::MEDIUM).text_color(T.text).child("Le prompt est un livrable"))
                        .child("Les prompts de l'équipe vivent dans .supacode/prompts/ : versionnés, relus et commentés comme du code, au lieu de disparaître avec un onglet.")
                        .child("Crée-en un avec le bouton ci-dessus, ou garde celui d'une conversation avec « Enregistrer le prompt »."),
                );
            }
            Ok(prompts) => {
                let selected = self.prompt_view.as_ref().filter(|_| self.right == RightPanel::Prompt).map(|v| v.id.clone());
                for (i, prompt) in prompts.iter().enumerate() {
                    let id = prompt.id.clone();
                    let launch = prompt.id.clone();
                    let vars = prompt.variables.iter().map(|v| tag(format!("{{{{{v}}}}}"))).collect::<Vec<_>>();
                    list = list.child(
                        div()
                            .id(("prompt", i))
                            .group(SharedString::from(format!("prompt-row-{i}")))
                            .px_1p5()
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .when(selected.as_deref() == Some(prompt.id.as_str()), |d| d.bg(T.hover))
                            .hover(|d| d.bg(T.hover))
                            .on_click(cx.listener(move |this, _, _, cx| this.open_prompt(id.clone(), cx)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.0))
                                    .child(div().truncate().child(prompt.title.clone()))
                                    .when_some(prompt.description.clone(), |d, desc| d.child(div().truncate().text_size(px(11.0)).text_color(T.text3).child(desc)))
                                    .when(!vars.is_empty(), |d| d.child(div().flex().flex_wrap().gap_1().children(vars))),
                            )
                            .child(
                                div()
                                    .id(("prompt-launch", i))
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
                                    .hover(|d| d.bg(T.raised).text_color(T.text))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.launch_prompt(&launch, window, cx);
                                    }))
                                    .child(icon(icons::PLAY).size(px(10.0))),
                            ),
                    );
                }
            }
        }
        list
    }

    pub(super) fn render_prompt_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        let view = self.prompt_view.as_ref()?;
        let project = self.workflows_project()?;
        let file = prompts::load(&project.path, &view.id).ok()?;
        let button = |id: &'static str, svg: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(28.0))
                .px_2p5()
                .flex()
                .items_center()
                .gap_1p5()
                .rounded(px(6.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .when(primary, |d| d.bg(T.accent_bg).text_color(T.accent_on).font_weight(FontWeight::MEDIUM).hover(|d| d.opacity(0.9)))
                .when(!primary, |d| d.border_1().border_color(T.border).text_color(T.text2).hover(|d| d.bg(T.hover).text_color(T.text)))
                .child(icon(svg).size(px(11.0)))
                .child(label)
        };
        let (launch, edit) = (view.id.clone(), view.id.clone());
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
                    .child(icon(icons::QUOTE).size(px(14.0)).text_color(T.accent))
                    .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(file.title.clone()))
                    .when(!view.version.is_empty(), |d| d.child(tag(view.version.clone())))
                    .child(
                        div()
                            .id("prompt-close")
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
            .when_some(file.description.clone(), |d, desc| d.child(div().text_size(px(12.0)).text_color(T.text2).child(desc)))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(button("prompt-run", icons::PLAY, "Lancer", true).on_click(cx.listener(move |this, _, window, cx| this.launch_prompt(&launch, window, cx))))
                    .child(button("prompt-edit", icons::EDIT, "Modifier", false).on_click(cx.listener(move |this, _, _, cx| this.edit_prompt(&edit, cx))))
                    .child(div().flex_1())
                    .child(div().text_size(px(11.0)).text_color(T.text3).font_family(theme::MONO_FONT).child(prompts::relative_path(&view.id))),
            );

        let body = match &view.compare {
            Some((commit, lines)) => div()
                .flex()
                .flex_col()
                .child(div().px_4().pt_3().pb_1().text_size(px(11.0)).text_color(T.text3).child(format!("Différences entre {commit} et la version actuelle")))
                .children(lines.iter().map(|(tag, line)| {
                    let (bg, sign, color) = match tag {
                        ChangeTag::Insert => (Some(T.add_bg), "+", T.add_fg),
                        ChangeTag::Delete => (Some(T.del_bg), "−", T.del_fg),
                        ChangeTag::Equal => (None, " ", T.text3),
                    };
                    div()
                        .flex()
                        .px_4()
                        .when_some(bg, |d, bg| d.bg(bg))
                        .child(div().w(px(16.0)).flex_shrink_0().text_color(color).child(sign))
                        .child(div().flex_1().min_w_0().child(if line.is_empty() { " ".to_owned() } else { line.clone() }))
                })),
            None => div().px_4().py_3().child(file.body.clone()),
        };

        let history = view.history.iter().enumerate().map(|(i, version)| {
            let commit = version.commit.clone();
            let selected = view.compare.as_ref().is_some_and(|(c, _)| *c == version.commit);
            div()
                .id(("prompt-version", i))
                .px_4()
                .py_1p5()
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .when(selected, |d| d.bg(T.hover))
                .hover(|d| d.bg(T.hover))
                .on_click(cx.listener(move |this, _, _, cx| this.compare_prompt(commit.clone(), cx)))
                .child(div().flex_shrink_0().font_family(theme::MONO_FONT).text_size(px(11.0)).text_color(T.accent).child(version.commit.clone()))
                .child(div().flex_1().min_w_0().truncate().text_size(px(12.0)).child(version.subject.clone()))
                .child(div().flex_shrink_0().text_size(px(11.0)).text_color(T.text3).child(format!("{} · {}", version.author, version.date)))
        });
        let history = div()
            .flex_shrink_0()
            .border_t_1()
            .border_color(T.border)
            .child(div().px_4().pt_3().pb_1().child(section_label("Historique")))
            .when(view.history.is_empty(), |d| {
                d.child(div().px_4().pb_3().text_size(px(12.0)).text_color(T.text3).child("Pas encore de version : committe le fichier pour garder son historique."))
            })
            .child(div().id("prompt-history").max_h(px(220.0)).overflow_y_scroll().pb_2().children(history))
            .when(!view.history.is_empty(), |d| d.child(div().px_4().pb_3().text_size(px(11.0)).text_color(T.text3).child("Clique une version pour la comparer à l'actuelle.")));

        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(header)
                .child(div().id("prompt-body").flex_1().min_h_0().overflow_y_scroll().font_family(theme::MONO_FONT).text_size(px(12.5)).line_height(px(20.0)).child(body))
                .child(history),
        )
    }
}
