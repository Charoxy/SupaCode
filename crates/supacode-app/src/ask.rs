//! Petite fenêtre de saisie avant un lancement : variables d'un prompt, description
//! d'un bug pour un workflow…

use std::collections::HashMap;

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::icons::{self, icon};
use crate::theme::{self, DARK as T};

pub enum AskEvent {
    Submit(HashMap<String, String>),
    Cancel,
}

impl EventEmitter<AskEvent> for AskDialog {}

pub struct AskDialog {
    title: SharedString,
    subtitle: Option<SharedString>,
    submit: SharedString,
    /// Clé, libellé, champ.
    fields: Vec<(String, String, Entity<TextareaState>)>,
    /// Choix proposés (valeur, libellé) ; la valeur choisie part sous la clé `choix`.
    choices: Vec<(String, String)>,
    choice: usize,
    choices_label: SharedString,
}

impl AskDialog {
    /// `fields` : (clé, libellé, valeur proposée).
    pub fn new(title: impl Into<SharedString>, subtitle: Option<String>, submit: impl Into<SharedString>, fields: Vec<(String, String, String)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let single = fields.len() == 1;
        let fields: Vec<_> = fields
            .into_iter()
            .map(|(key, label, value)| {
                let rows = if single { 5 } else { 2 };
                let state = cx.new(|cx| TextareaState::new(window, cx).rows(rows).default_value(value));
                (key, label, state)
            })
            .collect();
        if let Some((_, _, first)) = fields.first() {
            first.update(cx, |f, cx| f.focus(window, cx));
        }
        Self { title: title.into(), subtitle: subtitle.map(Into::into), submit: submit.into(), fields, choices: Vec::new(), choice: 0, choices_label: SharedString::default() }
    }

    /// Ajoute une liste de choix (le premier est sélectionné).
    pub fn with_choices(mut self, label: &str, choices: Vec<(String, String)>) -> Self {
        self.choices = choices;
        self.choices_label = label.to_owned().into();
        self
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let mut values: HashMap<String, String> = self.fields.iter().map(|(key, _, state)| (key.clone(), state.read(cx).value().trim().to_owned())).collect();
        if let Some((value, _)) = self.choices.get(self.choice) {
            values.insert("choix".into(), value.clone());
        }
        cx.emit(AskEvent::Submit(values));
    }
}

impl Render for AskDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fields = self.fields.iter().map(|(_, label, state)| {
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(div().text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(T.text2).child(label.clone()))
                .child(div().p_2().border_1().border_color(T.border).rounded(px(6.0)).bg(T.app).child(Textarea::new(state).appearance(false)))
        });
        div()
            .w(px(560.0))
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .border_1()
            .border_color(T.border_strong)
            .bg(T.surface)
            .shadow_lg()
            .font_family(theme::UI_FONT)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let k = &event.keystroke;
                if k.key == "escape" {
                    cx.emit(AskEvent::Cancel);
                } else if k.key == "enter" && k.modifiers.control {
                    this.submit(cx);
                }
            }))
            .child(
                div()
                    .px_5()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .border_b_1()
                    .border_color(T.border)
                    .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).child(self.title.clone()))
                    .when_some(self.subtitle.clone(), |d, s| d.child(div().text_size(px(12.0)).text_color(T.text3).child(s))),
            )
            .child(div().px_5().py_4().flex().flex_col().gap_3().children(fields).when(!self.choices.is_empty(), |d| {
                let chips = self.choices.iter().enumerate().map(|(i, (_, label))| {
                    let selected = i == self.choice;
                    div()
                        .id(("ask-choice", i))
                        .h(px(28.0))
                        .px_2p5()
                        .flex()
                        .items_center()
                        .rounded(px(6.0))
                        .text_size(px(12.0))
                        .cursor_pointer()
                        .border_1()
                        .when(selected, |d| d.bg(T.accent_bg).border_color(T.accent_bg).text_color(T.accent_on))
                        .when(!selected, |d| d.border_color(T.border).text_color(T.text2).hover(|d| d.bg(T.hover)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.choice = i;
                            cx.notify();
                        }))
                        .child(label.clone())
                });
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .child(div().text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(T.text2).child(self.choices_label.clone()))
                        .child(div().flex().flex_wrap().gap_1p5().children(chips)),
                )
            }))
            .child(
                div()
                    .px_5()
                    .py_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_t_1()
                    .border_color(T.border)
                    .child(div().flex_1().text_size(px(11.0)).text_color(T.text3).child("Ctrl+Entrée pour lancer, Échap pour annuler"))
                    .child(
                        div()
                            .id("ask-cancel")
                            .h(px(30.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(T.border)
                            .text_color(T.text2)
                            .cursor_pointer()
                            .hover(|d| d.bg(T.hover))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(AskEvent::Cancel)))
                            .child("Annuler"),
                    )
                    .child(
                        div()
                            .id("ask-submit")
                            .h(px(30.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .rounded(px(6.0))
                            .bg(T.accent_bg)
                            .text_color(T.accent_on)
                            .font_weight(FontWeight::MEDIUM)
                            .cursor_pointer()
                            .hover(|d| d.opacity(0.9))
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
                            .child(icon(icons::PLAY).size(px(10.0)))
                            .child(self.submit.clone()),
                    ),
            )
    }
}
