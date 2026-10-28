//! Onglet Équipe : règles d'équipe du projet (`.supacode/policy.json`), coûts estimés et
//! budgets, journal d'audit (événements de SupaCode et outils utilisés dans les conversations).

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use supacode_core::audit::{self, Entry, kind};
use supacode_core::costs::{self, Scanner, Totals};
use supacode_core::policy::{self, Policy};
use supacode_core::time::relative as relative_time;

use super::*;

/// Lignes du journal gardées en mémoire pour l'affichage.
const JOURNAL_LIMIT: usize = 3000;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum JournalFilter {
    All,
    Conversations,
    Commands,
    Agents,
    Blocked,
    Browser,
}

impl JournalFilter {
    const ALL: [JournalFilter; 6] = [JournalFilter::All, JournalFilter::Conversations, JournalFilter::Commands, JournalFilter::Agents, JournalFilter::Blocked, JournalFilter::Browser];

    fn label(self) -> &'static str {
        match self {
            JournalFilter::All => "Tout",
            JournalFilter::Conversations => "Conversations",
            JournalFilter::Commands => "Commandes",
            JournalFilter::Agents => "Agents",
            JournalFilter::Blocked => "Blocages",
            JournalFilter::Browser => "Navigateur",
        }
    }

    fn keeps(self, entry: &Entry) -> bool {
        let k = entry.kind.as_str();
        match self {
            JournalFilter::All => true,
            JournalFilter::Conversations => k == kind::CONVERSATION || k == kind::TOOL,
            JournalFilter::Commands => k == kind::COMMAND || (k == kind::TOOL && entry.summary.starts_with("Bash")),
            JournalFilter::Agents => k == kind::AGENT || k == kind::WORKFLOW || k == kind::APPROVAL,
            JournalFilter::Blocked => k == kind::BLOCKED,
            JournalFilter::Browser => k == kind::BROWSER,
        }
    }
}

/// Ce que l'onglet Équipe montre pour un projet.
#[derive(Default)]
pub(super) struct TeamData {
    pub root: Option<PathBuf>,
    pub policy: Option<Result<Policy, String>>,
    pub totals: Totals,
    /// Du plus récent au plus ancien.
    pub journal: Vec<Entry>,
}

/// Lit les règles, le journal et les transcripts du projet (à appeler hors du fil de l'interface).
pub(super) fn compute(scanner: &Mutex<Scanner>, root: &Path, claude_id: &str) -> TeamData {
    let policy = match policy::load(root) {
        Ok(Some(p)) => Some(Ok(p)),
        Ok(None) => None,
        Err(e) => Some(Err(format!("{e:#}"))),
    };
    let transcripts = match claude::projects_dir().filter(|_| !claude_id.is_empty()) {
        Some(dir) => scanner.lock().unwrap().scan(&dir.join(claude_id)),
        None => Vec::new(),
    };
    let mut journal = audit::read(root);
    let now = audit::now();
    let workflow_costs = journal.iter().filter_map(|e| {
        let model = e.details.get("modèle").and_then(|m| m.as_str()).unwrap_or("workflow");
        e.cost_usd.filter(|_| e.kind == kind::AGENT).map(|c| (e.time.as_str(), model, c))
    });
    let spends = transcripts.iter().flat_map(|t| t.spends.iter().map(|s| (s.time.as_str(), s.model.as_str(), s.cost)));
    let totals = costs::totals(spends.chain(workflow_costs).collect::<Vec<_>>(), &now[..10], &now[..7]);
    // Outils utilisés dans les conversations (Claude Code les enregistre dans ses transcripts).
    let user = audit::user(root);
    for transcript in &transcripts {
        for a in &transcript.activity {
            // Une commande sur plusieurs lignes tient sur une seule.
            let detail = a.detail.split_whitespace().collect::<Vec<_>>().join(" ");
            let summary = if detail.is_empty() { a.tool.clone() } else { format!("{} : {detail}", a.tool) };
            journal.push(Entry { time: a.time.clone(), user: user.clone(), kind: kind::TOOL.into(), summary, details: serde_json::json!({ "conversation": a.session }), cost_usd: None });
        }
    }
    journal.sort_by(|a, b| b.time.cmp(&a.time));
    journal.truncate(JOURNAL_LIMIT);
    TeamData { root: Some(root.to_owned()), policy, totals, journal }
}

fn money(value: f64) -> String {
    format!("{value:.2} $").replace('.', ",")
}

impl Workspace {
    /// Projet de l'onglet Équipe (le même que celui des workflows).
    fn team_project(&self) -> Option<Project> {
        self.workflows_project()
    }

    pub(super) fn refresh_team(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.team_project() else {
            self.team = TeamData::default();
            cx.notify();
            return;
        };
        let scanner = self.scanner.clone();
        let task = cx.spawn(async move |this, cx| {
            let data = cx.background_executor().spawn(async move { compute(&scanner, &project.path, &project.id) }).await;
            let _ = this.update(cx, |this, cx| {
                // Le projet a pu changer entre-temps.
                if this.team_project().is_some_and(|p| data.root.as_deref().is_some_and(|r| same_path(r, &p.path))) {
                    this.team = data;
                    cx.notify();
                }
            });
        });
        self._tasks.push(task);
    }

    /// Rafraîchit coûts et journal toutes les minutes (barre d'état, budgets).
    pub(super) fn watch_team(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(60)).await;
                if this.update(cx, |this, cx| this.refresh_team(cx)).is_err() {
                    break;
                }
            }
        });
        self._tasks.push(task);
    }

    /// Message bref dans la barre d'état.
    pub(super) fn toast(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast = Some((text.into(), std::time::Instant::now()));
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(8)).await;
            let _ = this.update(cx, |this, cx| {
                if this.toast.as_ref().is_some_and(|(_, at)| at.elapsed() >= Duration::from_secs(8)) {
                    this.toast = None;
                    cx.notify();
                }
            });
        });
        self._tasks.push(task);
    }

    /// Règles du projet pour lancer un agent ; `Err` si le fichier est invalide.
    pub(super) fn policy_for(&self, root: &Path) -> Result<Option<Policy>, String> {
        policy::load(root).map_err(|e| format!("{e:#}"))
    }

    pub(super) fn create_policy(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.team_project() else { return };
        let path = project.path.join(policy::FILE);
        if !path.exists() {
            let written = path.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&path, policy::template()));
            if let Err(e) = written {
                self.toast(format!("Impossible de créer les règles : {e}"), cx);
                return;
            }
            audit::record(&project.path, kind::POLICY, "Règles d'équipe créées", serde_json::Value::Null, None);
        }
        self.open_file(&project.path, policy::FILE.into(), cx);
        self.refresh_team(cx);
    }

    pub(super) fn edit_policy(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.team_project() else { return };
        let path = project.path.join(policy::FILE);
        // Ouvert avec l'éditeur associé aux fichiers JSON.
        let opened = std::process::Command::new("cmd").args(["/c", "start", ""]).arg(&path).spawn();
        if opened.is_err() {
            self.open_file(&project.path, policy::FILE.into(), cx);
        }
    }

    fn export_journal(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.team.root.clone() else { return };
        let mut entries: Vec<Entry> = self.team.journal.iter().filter(|e| self.journal_filter.keeps(e)).cloned().collect();
        entries.reverse();
        let file = root.join(".supacode").join("journal").join(format!("export-{}.csv", &audit::now()[..10]));
        match std::fs::create_dir_all(file.parent().unwrap_or(&root)).and_then(|_| std::fs::write(&file, audit::to_csv(&entries))) {
            Ok(()) => self.toast(format!("Journal exporté : {}", file.display()), cx),
            Err(e) => self.toast(format!("Export impossible : {e}"), cx),
        }
    }

    // — Rendu

    pub(super) fn render_team(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(project) = self.team_project() else {
            return div().p_3().text_size(px(12.0)).text_color(T.text3).child("Ajoute un projet pour voir ses règles, ses coûts et son journal.").into_any_element();
        };
        let card = || div().mx_2().mb_2().p_3().rounded(px(8.0)).border_1().border_color(T.border).bg(T.surface).flex().flex_col().gap_2();
        let line = |label: &str, value: String| {
            div().flex().items_start().gap_2().text_size(px(12.0)).child(div().w(px(118.0)).flex_shrink_0().text_color(T.text3).child(label.to_owned())).child(div().flex_1().min_w_0().child(value))
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(26.0))
                .px_2p5()
                .flex()
                .items_center()
                .rounded(px(6.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .when(primary, |d| d.bg(T.accent_bg).text_color(T.accent_on).hover(|d| d.opacity(0.9)))
                .when(!primary, |d| d.border_1().border_color(T.border).text_color(T.text2).hover(|d| d.bg(T.hover).text_color(T.text)))
                .child(label)
        };

        // Règles
        let mut rules = card().child(div().flex().items_center().gap_2().child(icon(icons::SHIELD).size(px(14.0)).text_color(T.accent)).child(div().font_weight(FontWeight::MEDIUM).child("Règles d'équipe")));
        match &self.team.policy {
            None => {
                rules = rules
                    .child(div().text_size(px(12.0)).text_color(T.text2).child(format!(
                        "Aucune règle pour {}. Elles vivent dans {}, versionné avec le code : toute l'équipe a les mêmes.",
                        project.name,
                        policy::FILE
                    )));
            }
            Some(Err(error)) => {
                rules = rules
                    .child(div().text_size(px(12.0)).text_color(T.err).child(format!("✖ {error}")))
                    .child(div().text_size(px(12.0)).text_color(T.text3).child("Les agents sont bloqués tant que le fichier n'est pas corrigé."));
            }
            Some(Ok(p)) => {
                let list = |items: &[String], all: &str| if items.is_empty() { all.to_owned() } else { items.join(", ") };
                let budget = [p.budget.daily_usd.map(|d| format!("{} / jour", money(d))), p.budget.monthly_usd.map(|m| format!("{} / mois", money(m))), p.budget.per_run_usd.map(|r| format!("{} / workflow", money(r)))]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();
                rules = rules
                    .child(line("Agents", list(&p.agents, "tous")))
                    .child(line("Modèles", list(&p.models, "tous (choix de l'orchestrateur)")))
                    .child(line("Commandes bloquées", if p.blocked_commands.is_empty() { "aucune".into() } else { p.blocked_commands.join("  ·  ") }))
                    .child(line("Fichiers protégés", list(&p.protected_files, "aucun")))
                    .child(line("Budgets", if budget.is_empty() { "aucun".into() } else { budget.join(", ") }))
                    .child(line("Navigateur", if p.browser { "permis aux agents".into() } else { "interdit aux agents".into() }))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(button("policy-edit", "Modifier", false).on_click(cx.listener(|this, _, _, cx| this.edit_policy(cx))))
                            .child(button("policy-view", "Voir le fichier", false).on_click(cx.listener(|this, _, _, cx| {
                                if let Some(project) = this.team_project() {
                                    this.open_file(&project.path, policy::FILE.into(), cx);
                                }
                            }))),
                    );
            }
        }

        // Coûts
        let totals = &self.team.totals;
        let budget = self.team.policy.as_ref().and_then(|p| p.as_ref().ok()).map(|p| p.budget.clone()).unwrap_or_default();
        let meter = |label: &str, used: f64, limit: Option<f64>| {
            let ratio = limit.map(|l| (used / l.max(0.01)).min(1.0) as f32);
            let over = limit.is_some_and(|l| used >= l);
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_size(px(12.0))
                        .child(div().text_color(T.text3).child(label.to_owned()))
                        .child(div().text_color(if over { T.err } else { T.text }).font_weight(FontWeight::MEDIUM).child(match limit {
                            Some(l) => format!("{} / {}", money(used), money(l)),
                            None => money(used),
                        })),
                )
                .when_some(ratio, |d, ratio| {
                    d.child(div().h(px(4.0)).rounded(px(2.0)).bg(T.raised).child(div().h_full().rounded(px(2.0)).w(gpui_kit::relative(ratio)).bg(if over { T.err } else if ratio > 0.8 { T.warn } else { T.accent })))
                })
        };
        let models = totals.by_model.iter().take(4).map(|(model, cost)| {
            div().flex().justify_between().text_size(px(11.0)).text_color(T.text3).font_family(theme::MONO_FONT).child(model.clone()).child(money(*cost))
        });
        let spending = card()
            .child(div().font_weight(FontWeight::MEDIUM).child("Coûts estimés"))
            .child(meter("Aujourd'hui", totals.today, budget.daily_usd))
            .child(meter("Ce mois", totals.month, budget.monthly_usd))
            .children(models)
            .child(div().text_size(px(11.0)).text_color(T.text3).child("Au prix public de l'API Claude (avec un abonnement, c'est une équivalence). Conversations et workflows de ce projet ; Codex n'est pas compté."));

        // Journal
        let blocked = self.team.journal.iter().filter(|e| e.kind == kind::BLOCKED).count();
        let journal = card()
            .child(div().font_weight(FontWeight::MEDIUM).child("Journal d'audit"))
            .child(div().text_size(px(12.0)).text_color(T.text2).child(format!(
                "{} événement(s) : qui a lancé quel agent, avec quelles commandes et quels fichiers.{}",
                self.team.journal.len(),
                if blocked > 0 { format!(" {blocked} blocage(s).") } else { String::new() }
)));

        div().id("team").flex_1().min_h_0().overflow_y_scroll().pt_1().flex().flex_col().child(rules).child(spending).child(journal).into_any_element()
    }

    pub(super) fn render_journal(&self, cx: &mut Context<Self>) -> Div {
        let chips = JournalFilter::ALL.into_iter().map(|filter| {
            let selected = self.journal_filter == filter;
            div()
                .id(SharedString::from(format!("journal-filter-{}", filter.label())))
                .h(px(24.0))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(12.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .when(selected, |d| d.bg(T.accent_bg).text_color(T.accent_on))
                .when(!selected, |d| d.border_1().border_color(T.border).text_color(T.text2).hover(|d| d.bg(T.hover)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.journal_filter = filter;
                    cx.notify();
                }))
                .child(filter.label())
        });
        let header = div()
            .flex_shrink_0()
            .px_3()
            .py_2()
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
                    .child(icon(icons::SHIELD).size(px(14.0)).text_color(T.accent))
                    .child(div().flex_1().font_weight(FontWeight::MEDIUM).child("Journal d'audit"))
                    .child(
                        div()
                            .id("journal-export")
                            .h(px(26.0))
                            .px_2p5()
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(T.border)
                            .text_size(px(12.0))
                            .text_color(T.text2)
                            .cursor_pointer()
                            .hover(|d| d.bg(T.hover).text_color(T.text))
                            .on_click(cx.listener(|this, _, _, cx| this.export_journal(cx)))
                            .child("Exporter (CSV)"),
                    )
                    .child(
                        div()
                            .id("journal-close")
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
            .child(div().flex().flex_wrap().gap_1p5().children(chips));

        let now = SystemTime::now();
        let rows: Vec<_> = self.team.journal.iter().filter(|e| self.journal_filter.keeps(e)).take(500).cloned().collect();
        let list = div().id("journal-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().children(rows.iter().enumerate().map(|(i, entry)| {
            let color = match entry.kind.as_str() {
                k if k == kind::BLOCKED => T.err,
                k if k == kind::AGENT || k == kind::WORKFLOW => T.accent,
                k if k == kind::COMMAND => T.warn,
                k if k == kind::TOOL || k == kind::CONVERSATION => T.claude,
                _ => T.text3,
            };
            let when = audit::parse(&entry.time).map(|t| relative_time(t, now)).unwrap_or_else(|| entry.time.clone());
            div()
                .id(("journal-row", i))
                .px_3()
                .py_1p5()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .border_b_1()
                .border_color(T.border)
                .when(entry.kind == kind::BLOCKED, |d| d.bg(rgba(0xE5484D14)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_shrink_0().px_1p5().rounded(px(4.0)).border_1().border_color(color).text_size(px(10.0)).text_color(color).child(entry.kind.clone()))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(12.0)).child(entry.summary.clone()))
                        .when_some(entry.cost_usd, |d, cost| d.child(div().flex_shrink_0().text_size(px(11.0)).text_color(T.text2).child(format!("{cost:.3} $").replace('.', ","))))
                )
                .child(div().text_size(px(11.0)).text_color(T.text3).child(format!("{when} · {}", entry.user)))
        }));
        let empty = rows.is_empty().then(|| div().p_6().text_size(px(12.0)).text_color(T.text3).child("Rien pour l'instant : le journal se remplit avec les conversations, les workflows et le navigateur."));
        div().size_full().flex().flex_col().child(header).children(empty).child(list)
    }
}
