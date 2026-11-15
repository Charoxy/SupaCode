//! Relire et rattraper le travail des agents : points de retour automatiques avant chaque
//! message, diff côte à côte, commentaire sur une ligne renvoyé à l'agent, et « Corriger
//! avec l'agent » sur une commande qui échoue.

use supacode_core::audit::{self, kind};
use supacode_core::git::{self, Checkpoint};

use crate::preview::{ChangedFile, DiffLine};

use super::*;

/// Ligne commentée dans le panneau de code.
pub(super) struct LineComment {
    pub file: String,
    /// Index de la ligne dans `ChangedFile::lines`.
    pub index: usize,
}

impl Workspace {
    /// Envoie un texte à la conversation ouverte sur ce projet, ou en ouvre une.
    pub(super) fn send_to_agent(&mut self, root: &Path, text: String, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_tab().filter(|t| same_path(&t.project.path, root) && !t.terminal.read(cx).exited) {
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
            None => {
                if let Some(project) = self.projects.iter().find(|p| same_path(&p.path, root)).cloned() {
                    self.open_conversation(project, None, Some(text), window, cx);
                }
            }
        }
    }

    // — Points de retour

    /// Avant que l'agent traite un message : état des fichiers, pour pouvoir y revenir.
    pub(super) fn take_checkpoint(&mut self, tab_id: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) else { return };
        let (root, group) = (tab.project.path.clone(), tab.conversation_id.clone());
        if !root.join(".git").exists() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let label = "avant ton message".to_owned();
            let (root2, group2) = (root.clone(), group.clone());
            let saved = cx.background_executor().spawn(async move { git::checkpoint(&root2, &group2, &label).map(|_| git::checkpoints(&root2, &group2)) }).await;
            if let Ok(list) = saved {
                let _ = this.update(cx, |this, cx| {
                    if this.active_tab().is_some_and(|t| t.conversation_id == group) {
                        this.checkpoint_list = list;
                        cx.notify();
                    }
                });
            }
        });
        self._tasks.push(task);
    }

    pub(super) fn toggle_checkpoints(&mut self, cx: &mut Context<Self>) {
        self.checkpoints_open = !self.checkpoints_open;
        if self.checkpoints_open {
            self.load_checkpoints(cx);
        }
        cx.notify();
    }

    /// Points de retour de l'onglet actif, lus en arrière-plan (git).
    pub(super) fn load_checkpoints(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else { return };
        let (root, group) = (tab.project.path.clone(), tab.conversation_id.clone());
        cx.spawn(async move |this, cx| {
            let group2 = group.clone();
            let list = cx.background_executor().spawn(async move { git::checkpoints(&root, &group2) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.active_tab().is_some_and(|t| t.conversation_id == group) {
                    this.checkpoint_list = list;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn restore_checkpoint(&mut self, checkpoint: Checkpoint, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else { return };
        let (root, group) = (tab.project.path.clone(), tab.conversation_id.clone());
        let changed = git::changed_since(&root, &checkpoint.commit).unwrap_or_default();
        if changed.is_empty() {
            self.toast("Aucun fichier n'a changé depuis ce point.", cx);
            return;
        }
        let shown: Vec<&str> = changed.iter().take(8).map(String::as_str).collect();
        let detail = format!(
            "{} fichier(s) reviendront à leur état d'avant :\n{}{}\n\nL'état actuel est gardé comme point de retour : tu peux annuler. La conversation, elle, ne revient pas en arrière.",
            changed.len(),
            shown.join("\n"),
            if changed.len() > shown.len() { "\n…" } else { "" }
        );
        let answer = window.prompt(PromptLevel::Warning, "Revenir à ce point ?", Some(&detail), &["Revenir", "Annuler"], cx);
        let task = cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let (root2, group2, commit) = (root.clone(), group.clone(), checkpoint.commit.clone());
            let result = cx.background_executor().spawn(async move { git::restore(&root2, &group2, &commit).map(|_| git::checkpoints(&root2, &group2)) }).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(list) => {
                        audit::record(&root, kind::CONVERSATION, format!("Retour en arrière : {} fichier(s) restauré(s)", changed.len()), serde_json::json!({ "conversation": group, "point": checkpoint.commit }), None);
                        this.checkpoint_list = list;
                        this.checkpoints_open = false;
                        this.toast(format!("{} fichier(s) restauré(s). L'état d'avant est gardé dans les points de retour.", changed.len()), cx);
                    }
                    Err(e) => this.toast(format!("Retour impossible : {e:#}"), cx),
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    pub(super) fn render_checkpoints(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.checkpoints_open {
            return None;
        }
        let now = SystemTime::now();
        let rows = self.checkpoint_list.iter().enumerate().map(|(i, point)| {
            let chosen = point.clone();
            let when = audit::parse(&point.time).map(|t| relative(t, now)).unwrap_or_else(|| point.time.clone());
            div()
                .id(("checkpoint", i))
                .h(px(30.0))
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .hover(|d| d.bg(T.hover))
                .child(div().w(px(84.0)).flex_shrink_0().text_size(px(12.0)).text_color(T.text3).child(when))
                .child(div().flex_1().min_w_0().truncate().text_size(px(12.0)).child(point.label.clone()))
                .child(
                    div()
                        .id(("checkpoint-restore", i))
                        .h(px(22.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(T.border_strong)
                        .text_size(px(11.0))
                        .cursor_pointer()
                        .hover(|d| d.bg(T.raised))
                        .on_click(cx.listener(move |this, _, window, cx| this.restore_checkpoint(chosen.clone(), window, cx)))
                        .child("Revenir ici"),
                )
        });
        Some(
            div()
                .flex_shrink_0()
                .max_h(px(240.0))
                .border_b_1()
                .border_color(T.border)
                .bg(T.panel)
                .flex()
                .flex_col()
                .child(div().px_3().pt_2().pb_1().text_size(px(11.0)).text_color(T.text3).child("Un point est pris à chaque message envoyé : revenir restaure les fichiers du projet à cet instant."))
                .when(self.checkpoint_list.is_empty(), |d| d.child(div().px_3().pb_2().text_size(px(12.0)).text_color(T.text3).child("Aucun point pour l'instant : le premier sera pris à ton prochain message.")))
                .child(div().id("checkpoint-list").overflow_y_scroll().pb_1().children(rows)),
        )
    }

    // — Diff : lignes, côte à côte, commentaires

    /// Lignes du fichier affiché. Liste virtuelle : seules les lignes visibles sont construites,
    /// même pour un fichier de plusieurs milliers de lignes.
    pub(super) fn render_diff_list(&self, file: &ChangedFile, mode: PreviewMode, cx: &mut Context<Self>) -> UniformList {
        let relative = file.relative.clone();
        // Ligne la plus longue : elle donne la largeur de la liste (défilement horizontal).
        let widest = |indices: &mut dyn Iterator<Item = (usize, usize)>| indices.max_by_key(|(_, line)| file.lines[*line].text.len()).map(|(row, _)| row);
        if mode == PreviewMode::Split {
            let rows = split_rows(&file.lines);
            let measure = widest(&mut rows.iter().enumerate().filter_map(|(row, (left, right))| right.or(*left).map(|i| (row, i))));
            let count = rows.len();
            return uniform_list(
                "code",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    let Some(file) = this.shown_file(&relative) else { return Vec::new() };
                    rows[range.clone()].iter().zip(range).map(|(&(left, right), row)| this.split_row(&file, row, left, right, cx)).collect()
                }),
            )
            .with_width_from_item(measure)
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained);
        }
        let indices: Vec<usize> = file.lines.iter().enumerate().filter(|(_, l)| mode == PreviewMode::Diff || l.kind != LineKind::Removed).map(|(i, _)| i).collect();
        let measure = widest(&mut indices.iter().copied().enumerate());
        uniform_list(
            "code",
            indices.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let Some(file) = this.shown_file(&relative) else { return Vec::new() };
                indices[range].iter().map(|&index| this.diff_row(&file, index, cx)).collect()
            }),
        )
        .with_width_from_item(measure)
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
    }

    /// Fichier modifié affiché dans le panneau de code.
    fn shown_file(&self, relative: &str) -> Option<std::rc::Rc<ChangedFile>> {
        self.active_tab()?.changed.iter().find(|f| f.relative == relative).cloned()
    }

    fn split_row(&self, file: &ChangedFile, row: usize, left: Option<usize>, right: Option<usize>, cx: &mut Context<Self>) -> AnyElement {
        let index = right.or(left).unwrap_or(0);
        let side = |line: Option<usize>| {
            let line = line.and_then(|i| file.lines.get(i));
            let (bg, gutter) = match line.map(|l| l.kind) {
                Some(LineKind::Added) => (Some(T.add_bg), Some(T.add_gutter)),
                Some(LineKind::Removed) => (Some(T.del_bg), Some(T.del_gutter)),
                _ => (None, None),
            };
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .flex()
                .when_some(bg, |d, bg| d.bg(bg))
                .when(line.is_none(), |d| d.bg(T.panel))
                .child(div().w(px(40.0)).flex_shrink_0().pr_2().flex().justify_end().text_color(T.text3).when_some(gutter, |d, g| d.bg(g)).child(line.and_then(|l| l.number).map(|n| n.to_string()).unwrap_or_default()))
                .child(div().pl_1().whitespace_nowrap().children(line.map(|l| code_text(&l.text, &l.spans))))
        };
        div()
            .id(("split-row", row))
            .flex()
            .h(px(20.0))
            .cursor_pointer()
            .hover(|d| d.opacity(0.9))
            .when(self.is_commenting(file, index), |d| d.border_l_2().border_color(T.accent))
            .on_click(self.comment_listener(file, index, cx))
            .child(side(left))
            .child(div().w(px(1.0)).flex_shrink_0().bg(T.border))
            .child(side(right))
            .into_any_element()
    }

    fn diff_row(&self, file: &ChangedFile, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let line = &file.lines[index];
        let (bg, gutter, sign, sign_color) = match line.kind {
            LineKind::Added => (Some(T.add_bg), Some(T.add_gutter), "+", T.add_fg),
            LineKind::Removed => (Some(T.del_bg), Some(T.del_gutter), "−", T.del_fg),
            LineKind::Same => (None, None, " ", T.text3),
        };
        div()
            .id(("code-line", index))
            .flex()
            .h(px(20.0))
            .cursor_pointer()
            .when_some(bg, |d, bg| d.bg(bg))
            .hover(|d| d.bg(T.hover))
            .when(self.is_commenting(file, index), |d| d.bg(T.hover))
            .on_click(self.comment_listener(file, index, cx))
            .child(div().w(px(48.0)).flex_shrink_0().pr_2().flex().justify_end().text_color(T.text3).when_some(gutter, |d, g| d.bg(g)).child(line.number.map(|n| n.to_string()).unwrap_or_default()))
            .child(div().w(px(18.0)).flex_shrink_0().text_center().text_color(sign_color).child(sign))
            .child(div().whitespace_nowrap().child(code_text(&line.text, &line.spans)))
            .into_any_element()
    }

    fn is_commenting(&self, file: &ChangedFile, index: usize) -> bool {
        self.line_comment.as_ref().is_some_and(|c| c.file == file.relative && c.index == index)
    }

    fn comment_listener(&self, file: &ChangedFile, index: usize, cx: &mut Context<Self>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let relative = file.relative.clone();
        cx.listener(move |this, _, window, cx| {
            let same = this.line_comment.as_ref().is_some_and(|c| c.file == relative && c.index == index);
            this.line_comment = (!same).then(|| LineComment { file: relative.clone(), index });
            if !same {
                this.comment_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus(window, cx);
                });
            }
            cx.notify();
        })
    }

    /// Champ du commentaire de la ligne choisie, sous la liste (les lignes gardent toutes la
    /// même hauteur, ce qu'exige la liste virtuelle).
    pub(super) fn render_comment_box(&self, file: &ChangedFile, cx: &mut Context<Self>) -> Option<Div> {
        let comment = self.line_comment.as_ref().filter(|c| c.file == file.relative)?;
        let line = file.lines.get(comment.index)?;
        let place = match (line.kind, line.number) {
            (LineKind::Removed, _) => "Ligne supprimée".to_owned(),
            (_, Some(n)) => format!("Ligne {n}"),
            _ => "Ligne".to_owned(),
        };
        Some(
            div()
                .flex_shrink_0()
                .mx_3()
                .my_1p5()
                .p_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(6.0))
                .border_1()
                .border_color(T.accent)
                .bg(T.raised)
                .font_family(theme::UI_FONT)
                .child(div().flex_shrink_0().text_size(px(12.0)).text_color(T.text3).child(place))
                .child(div().flex_1().min_w_0().child(Input::new(&self.comment_input).appearance(false)))
                .child(
                    div()
                        .id("line-comment-send")
                        .h(px(24.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(px(5.0))
                        .bg(T.accent_bg)
                        .text_color(T.accent_on)
                        .text_size(px(12.0))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.send_line_comment(window, cx)))
                        .child("Envoyer à l'agent"),
                ),
        )
    }

    /// Commentaire sur une ligne : part dans la conversation, avec le fichier et la ligne.
    pub(super) fn send_line_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(comment) = self.line_comment.take() else { return };
        let text = self.comment_input.read(cx).value().trim().to_owned();
        let Some(tab) = self.active_tab() else { return };
        let Some(file) = tab.changed.iter().find(|f| f.relative == comment.file) else { return };
        let Some(line) = file.lines.get(comment.index) else { return };
        if text.is_empty() {
            self.line_comment = Some(comment);
            return;
        }
        let place = match (line.kind, line.number) {
            (LineKind::Removed, _) => "ligne supprimée".to_owned(),
            (_, Some(n)) => format!("ligne {n}"),
            _ => "ligne".to_owned(),
        };
        let message = format!("Dans `{}`, {place} :\n```\n{}\n```\n{text}", comment.file, line.text.trim_end());
        let root = tab.project.path.clone();
        audit::record(&root, kind::CONVERSATION, format!("Commentaire de relecture sur {} ({place})", comment.file), serde_json::Value::Null, None);
        self.send_to_agent(&root, message, window, cx);
        cx.notify();
    }

    /// « Corriger avec l'agent » : la commande, sa sortie, et la consigne de corriger la cause.
    pub(super) fn fix_with_agent(&mut self, root: PathBuf, workflow: String, step: String, command: String, output: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let start = output.len().saturating_sub(80);
        let text = format!(
            "La commande `{command}` (étape « {step} » du workflow « {workflow} ») échoue. Fin de sa sortie :\n```\n{}\n```\nTrouve la cause et corrige-la. Ne désactive ni n'affaiblis aucun test pour le faire passer. Relance la commande pour vérifier.",
            output[start..].join("\n")
        );
        audit::record(&root, kind::AGENT, format!("Correction demandée à l'agent : {command}"), serde_json::Value::Null, None);
        self.send_to_agent(&root, text, window, cx);
    }
}

/// Lignes du diff appariées pour l'affichage côte à côte : (avant, après), en indices.
pub(super) fn split_rows(lines: &[DiffLine]) -> Vec<(Option<usize>, Option<usize>)> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind == LineKind::Same {
            rows.push((Some(i), Some(i)));
            i += 1;
            continue;
        }
        // Un bloc de changements : les suppressions à gauche, les ajouts à droite, alignés.
        let (mut removed, mut added) = (Vec::new(), Vec::new());
        while i < lines.len() && lines[i].kind != LineKind::Same {
            if lines[i].kind == LineKind::Removed { removed.push(i) } else { added.push(i) }
            i += 1;
        }
        for k in 0..removed.len().max(added.len()) {
            rows.push((removed.get(k).copied(), added.get(k).copied()));
        }
    }
    rows
}
