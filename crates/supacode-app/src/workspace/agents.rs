//! Les fenêtres d'agents entre elles : qui travaille, qui attend, et les outils MCP qui
//! permettent à un agent de voir les autres fenêtres et de leur écrire.

use serde_json::{Value, json};

use super::*;
use crate::mcp::Content;

/// Ce que fait une fenêtre d'agent en ce moment.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum AgentState {
    /// Claude réfléchit ou travaille.
    Working,
    /// Claude attend une réponse (permission, question à choix).
    Waiting,
    /// Tâche finie, prêt pour la suite.
    Idle,
    /// Le programme s'est arrêté.
    Exited,
}

impl AgentState {
    pub fn label(self) -> &'static str {
        match self {
            AgentState::Working => "au travail",
            AgentState::Waiting => "attend ta réponse",
            AgentState::Idle => "au repos",
            AgentState::Exited => "terminée",
        }
    }

    pub fn color(self) -> Rgba {
        match self {
            AgentState::Working => T.claude,
            AgentState::Waiting => T.warn,
            AgentState::Idle => T.ok,
            AgentState::Exited => T.text3,
        }
    }
}

/// Ce que Claude Code affiche quand il attend une décision de l'utilisateur.
fn asks_for_input(screen: &str) -> bool {
    const PATTERNS: [&str; 6] = ["Do you want to", "Would you like to", "Enter to select", "Enter to confirm", "❯ 1.", "Esc to cancel"];
    PATTERNS.iter().any(|p| screen.contains(p))
}

pub(super) fn agent_state(tab: &Tab, cx: &App) -> AgentState {
    let terminal = tab.terminal.read(cx);
    if terminal.exited {
        return AgentState::Exited;
    }
    if asks_for_input(&terminal.screen_text()) {
        return AgentState::Waiting;
    }
    if tab.busy_title || is_active(tab) {
        return AgentState::Working;
    }
    AgentState::Idle
}

/// En deçà, un passage « au travail » est trop bref pour mériter une notification
/// (un écho de frappe, un redessin).
const WORKED_ENOUGH: Duration = Duration::from_secs(4);
/// Après une frappe, l'utilisateur est peut-être encore en train d'écrire.
const USER_TYPING: Duration = Duration::from_secs(3);
/// Lignes d'écran renvoyées par `agent_read`.
const READ_LINES: usize = 40;

impl Workspace {
    /// Suit l'état des agents ; prévient (notification Windows) quand l'un d'eux a fini ou
    /// attend une réponse, si SupaCode n'est pas au premier plan.
    pub(super) fn watch_agents(&mut self, window_active: bool, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut notices = Vec::new();
        for tab in &self.tabs {
            let state = agent_state(tab, cx);
            let previous = self.agent_states.get(&tab.id).copied();
            if previous.is_some_and(|(s, _)| s == state) {
                continue;
            }
            changed = true;
            if let Some((AgentState::Working, since)) = previous {
                if since.elapsed() >= WORKED_ENOUGH && matches!(state, AgentState::Idle | AgentState::Waiting) && !window_active {
                    notices.push((tab.display_title(), tab.project.name.clone(), tab.id, state));
                }
            }
            self.agent_states.insert(tab.id, (state, Instant::now()));
        }
        self.agent_states.retain(|id, _| self.tabs.iter().any(|t| t.id == *id));
        let waiting: Vec<usize> = self.tabs.iter().filter(|t| !t.inbox.is_empty()).map(|t| t.id).collect();
        for id in waiting {
            changed |= self.deliver_messages(id, cx);
        }
        for (title, project, id, state) in notices {
            let heading = match state {
                AgentState::Waiting => format!("L'agent #{id} attend ta réponse"),
                _ => format!("L'agent #{id} a fini"),
            };
            crate::desktop::notify(&heading, &format!("{title} · {project}"));
        }
        if changed {
            cx.notify();
        }
    }

    pub(super) fn toggle_agents_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.right = if self.right == RightPanel::Agents { RightPanel::Code } else { RightPanel::Agents };
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(super) fn render_agents_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let states: Vec<(usize, AgentState)> = self.tabs.iter().map(|t| (t.id, agent_state(t, cx))).collect();
        let count = |state: AgentState| states.iter().filter(|(_, s)| *s == state).count();
        let summary = [AgentState::Working, AgentState::Waiting, AgentState::Idle]
            .into_iter()
            .filter(|s| count(*s) > 0)
            .map(|s| format!("{} {}", count(s), s.label()))
            .collect::<Vec<_>>()
            .join(" · ");

        let mut list = div().id("agents-list").flex_1().min_h_0().overflow_y_scroll().p_2().flex().flex_col().gap_1();
        if self.tabs.is_empty() {
            list = list.child(div().p_4().text_color(T.text3).child("Aucun agent ouvert."));
        }
        // Ceux qui attendent d'abord : ce sont eux qui ont besoin de toi.
        let mut order: Vec<&Tab> = self.tabs.iter().collect();
        let rank = |state: AgentState| match state {
            AgentState::Waiting => 0,
            AgentState::Working => 1,
            AgentState::Idle => 2,
            AgentState::Exited => 3,
        };
        order.sort_by_key(|t| states.iter().find(|(id, _)| *id == t.id).map_or(4, |(_, s)| rank(*s)));
        for tab in order {
            let id = tab.id;
            let state = states.iter().find(|(i, _)| *i == id).map_or(AgentState::Idle, |(_, s)| *s);
            let selected = Some(id) == self.active;
            let since = tab.last_output.map(|t| {
                let secs = t.elapsed().as_secs();
                if secs < 60 { format!("il y a {secs} s") } else { format!("il y a {} min", secs / 60) }
            });
            list = list.child(
                div()
                    .id(("agent-row", id))
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .px_2p5()
                    .py_2()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(if selected { T.claude } else { T.border })
                    .cursor_pointer()
                    .hover(|d| d.bg(T.hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.grid_zoomed = false;
                        this.activate(id, window, cx);
                    }))
                    .child(
                        div()
                            .size(px(8.0))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(state.color())
                            .opacity(if state == AgentState::Working && !self.pulse { 0.35 } else { 1.0 }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(div().truncate().text_color(T.text).child(format!("#{id} · {}", tab.display_title())))
                            .child(
                                div()
                                    .flex()
                                    .gap_1p5()
                                    .text_size(px(11.0))
                                    .child(div().text_color(state.color()).child(state.label()))
                                    .child(div().text_color(T.text2).child(super::model_label(tab.agent, tab.model.as_deref())))
                                    .child(div().text_color(T.text3).truncate().child(tab.project.name.clone()))
                                    .when_some(since.filter(|_| state != AgentState::Working), |d, since| d.child(div().text_color(T.text3).child(since))),
                            ),
                    ),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(36.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .border_b_1()
                    .border_color(T.border)
                    .bg(T.panel)
                    .child(icon(icons::AGENTS).size(px(13.0)).text_color(T.text2))
                    .child(div().font_weight(FontWeight::MEDIUM).child(format!("Agents · {}", self.tabs.len())))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(11.0)).text_color(T.text3).child(summary))
                    .child(
                        div()
                            .id("agents-close")
                            .size(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|d| d.bg(T.hover))
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_agents_panel(window, cx)))
                            .child(icon(icons::CLOSE).size(px(12.0)).text_color(T.text3)),
                    ),
            )
            .child(list)
    }

    /// Remet les messages en attente de l'agent si personne n'écrit dans sa fenêtre :
    /// l'agent est au repos (il ne travaille pas, ne pose pas de question) et l'utilisateur
    /// n'a ni saisie en cours ni frappe récente. Sinon le collage se mélangerait à ce que
    /// l'utilisateur tape, et Entrée enverrait son message tronqué.
    pub(super) fn deliver_messages(&mut self, id: usize, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return false };
        if tab.inbox.is_empty() || agent_state(tab, cx) != AgentState::Idle {
            return false;
        }
        let terminal = tab.terminal.read(cx);
        if terminal.draft || terminal.last_key.is_some_and(|t| t.elapsed() < USER_TYPING) {
            return false;
        }
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else { return false };
        let text = std::mem::take(&mut tab.inbox).join("\n\n");
        let terminal = tab.terminal.clone();
        terminal.read(cx).paste(&text);
        // Laisse Claude Code recevoir le collage avant de valider.
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            let _ = terminal.update(cx, |t, _| {
                // L'utilisateur s'est mis à écrire entre-temps : il validera lui-même.
                if !t.draft {
                    t.write(&b"\r"[..]);
                }
            });
        })
        .detach();
        true
    }

    /// Consigne ajoutée aux agents lancés par SupaCode : ils savent qu'ils ne sont pas seuls.
    pub(super) fn agent_prompt(&self, id: usize) -> String {
        let others: Vec<String> = self.tabs.iter().map(|t| format!("#{} « {} » ({})", t.id, t.display_title().replace('"', "'"), t.project.name)).collect();
        let others = if others.is_empty() { "aucune pour l'instant".to_owned() } else { others.join(", ") };
        format!(
            "Tu tournes dans SupaCode, dans la fenêtre d'agent #{id}. D'autres agents Claude Code peuvent travailler en parallèle \
             dans d'autres fenêtres, parfois sur le même projet. Fenêtres déjà ouvertes à ton lancement : {others}. \
             La liste change : l'outil mcp__supacode__agents_list donne l'état à jour (qui travaille, qui attend). \
             mcp__supacode__agent_send écrit à un autre agent, mcp__supacode__agent_read montre son écran. \
             Écris seulement si ça change son travail (fichier que tu modifies, conflit, résultat dont il a besoin) : \
             une ou deux phrases factuelles, sans politesse, remerciement ni accusé de réception. \
             Ne réponds à un message que si tu as quelque chose d'utile à dire. Les messages reçus arrivent en direct \
             pendant ta tâche : adapte-toi aussitôt (ne refais pas ce qu'un autre vient de faire). \
             Pour une grosse tâche faite de parties indépendantes, mcp__supacode__agents_spawn crée des sous-agents \
             dans la grille (de vrais terminaux que l'utilisateur voit et pilote), chacun avec le modèle adapté : \
             préfère-le à tes sous-agents internes quand l'utilisateur doit pouvoir suivre ou corriger leur travail."
        )
    }

    /// Outil `agents_spawn` : ouvre des sous-agents dans la grille, chacun dans son terminal.
    /// L'agent et le modèle de chacun viennent de l'orchestrateur du projet (sauf choix imposé),
    /// ramenés dans les règles d'équipe. Ils ne prennent pas le focus.
    pub(super) fn spawn_agents(&mut self, caller: Option<usize>, args: &Value, window: &mut Window, cx: &mut Context<Self>) -> anyhow::Result<Vec<Content>> {
        use supacode_core::router::{self, AgentId};
        let list = args.get("agents").and_then(Value::as_array).filter(|a| !a.is_empty()).ok_or_else(|| anyhow::anyhow!("indique les sous-agents dans « agents » (une tâche chacun)"))?;
        if list.len() > crate::mcp::MAX_SPAWN {
            anyhow::bail!("{} sous-agents demandés, {} au plus par appel : regroupe les petites tâches", list.len(), crate::mcp::MAX_SPAWN);
        }
        let project = caller
            .and_then(|c| self.tabs.iter().find(|t| t.id == c))
            .map(|t| t.project.clone())
            .or_else(|| self.current_project())
            .ok_or_else(|| anyhow::anyhow!("aucun projet ouvert dans SupaCode"))?;
        let config = router::load_config(&project.path).unwrap_or_default();
        let policy = self.policy_for(&project.path).map_err(|e| anyhow::anyhow!("règles d'équipe invalides : {e}"))?;

        // Tout est vérifié avant de lancer quoi que ce soit : pas de grille à moitié créée.
        let mut plans = Vec::new();
        for (index, item) in list.iter().enumerate() {
            let task = item.get("task").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| anyhow::anyhow!("sous-agent {} : « task » est vide", index + 1))?;
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| t.chars().take(60).collect::<String>())
                .unwrap_or_else(|| task.chars().take(48).collect());
            let forced = match item.get("agent").and_then(Value::as_str).map(str::to_lowercase).as_deref() {
                Some("claude") => Some(AgentId::Claude),
                Some("codex") => Some(AgentId::Codex),
                _ => None,
            };
            let model = item.get("model").and_then(Value::as_str).map(str::trim).filter(|m| !m.is_empty() && !m.eq_ignore_ascii_case("auto"));
            let mut decision = router::route(task, &config, forced, model);
            // Codex absent de la machine : Claude prend la tâche, avec le modèle prévu pour lui.
            if decision.agent == AgentId::Codex && !self.codex_installed {
                decision = router::route(task, &config, Some(AgentId::Claude), model);
                decision.reason.push_str(" (Codex n'est pas installé)");
            }
            if let Some(policy) = &policy {
                policy.constrain(&mut decision).map_err(|e| anyhow::anyhow!("sous-agent « {title} » : {e}"))?;
            }
            plans.push((task.to_owned(), title, decision));
        }

        let parent = caller.and_then(|c| self.tabs.iter().find(|t| t.id == c)).map(|t| format!("#{} « {} »", t.id, t.display_title()));
        let mut lines = Vec::new();
        for (task, title, decision) in plans {
            let report = match (caller, decision.agent) {
                (Some(c), AgentId::Claude) => format!(
                    " Quand tu as fini, ou si tu es bloqué, envoie le résultat en une ou deux phrases à l'agent #{c} avec mcp__supacode__agent_send (to={c})."
                ),
                _ => String::new(),
            };
            let brief = format!(
                "Tu es un sous-agent{} chargé de : {title}. L'utilisateur voit ta fenêtre dans la grille de SupaCode et peut t'écrire \
                 directement : ses consignes priment sur celles de l'agent qui t'a créé. Reste dans ton périmètre ; si une autre \
                 fenêtre travaille sur les mêmes fichiers, coordonne-toi avec elle.{report}",
                parent.as_ref().map(|p| format!(" créé par l'agent {p}")).unwrap_or_default()
            );
            let launch = super::Launch {
                agent: Some(decision.agent),
                model: decision.model.clone(),
                title: Some(title.clone()),
                parent: caller,
                brief: Some(brief),
                background: true,
            };
            let label = super::model_label(decision.agent, decision.model.as_deref());
            match self.open_conversation_checked(project.clone(), None, Some(task), launch, true, window, cx) {
                Some(id) => lines.push(format!("- #{id} « {title} » : {label} ({})", decision.reason)),
                None => lines.push(format!("- « {title} » : non lancé (règles d'équipe)")),
            }
        }
        // Les sous-agents se voient dans la grille.
        self.layout.grid = true;
        self.grid_zoomed = false;
        super::save_layout(self.layout);
        cx.notify();
        let follow = if caller.is_some() { " Ils t'enverront leur résultat avec agent_send ;" } else { "" };
        Ok(vec![Content::Text(format!(
            "Sous-agents créés dans la grille de SupaCode :\n{}\n\nL'utilisateur les voit et peut les piloter.{follow} agents_list et agent_read montrent où ils en sont.",
            lines.join("\n")
        ))])
    }

    /// Outils `agents_*` : exécutés ici, dans l'interface.
    pub(super) fn agent_tool(&mut self, caller: Option<usize>, tool: &str, args: &Value, cx: &mut Context<Self>) -> anyhow::Result<Vec<Content>> {
        let target = |key: &str| args.get(key).and_then(Value::as_u64).map(|v| v as usize);
        match tool {
            "agents_list" => {
                let mut text = format!("{} fenêtre(s) d'agents ouvertes dans SupaCode :\n", self.tabs.len());
                for tab in &self.tabs {
                    let me = if Some(tab.id) == caller { " (toi)" } else { "" };
                    let state = agent_state(tab, cx).label();
                    let model = super::model_label(tab.agent, tab.model.as_deref());
                    let parent = tab.parent.map(|p| format!(", sous-agent de #{p}")).unwrap_or_default();
                    text.push_str(&format!("- #{}{me} « {} » : {state}. {model}{parent}. Projet {} ({})\n", tab.id, tab.display_title(), tab.project.name, tab.project.path.display()));
                }
                Ok(vec![Content::Text(text)])
            }
            "agent_read" => {
                let id = target("id").ok_or_else(|| anyhow::anyhow!("indique l'id de la fenêtre (voir agents_list)"))?;
                let tab = self.tabs.iter().find(|t| t.id == id).ok_or_else(|| anyhow::anyhow!("aucune fenêtre #{id}"))?;
                // Les dernières lignes non vides suffisent : l'écran entier coûterait des tokens pour rien.
                let screen = tab.terminal.read(cx).screen_text();
                let lines: Vec<&str> = screen.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
                let screen = lines[lines.len().saturating_sub(READ_LINES)..].join("\n");
                Ok(vec![Content::Text(format!("Écran de l'agent #{id} « {} » ({}) :\n\n{screen}", tab.display_title(), agent_state(tab, cx).label()))])
            }
            "agent_send" => {
                let id = target("to").ok_or_else(|| anyhow::anyhow!("indique l'id du destinataire dans « to » (voir agents_list)"))?;
                let message = args.get("message").and_then(Value::as_str).map(str::trim).filter(|m| !m.is_empty()).ok_or_else(|| anyhow::anyhow!("message vide"))?;
                if Some(id) == caller {
                    anyhow::bail!("tu ne peux pas t'écrire à toi-même");
                }
                let length = message.chars().count();
                if length > crate::mcp::MAX_MESSAGE {
                    anyhow::bail!("message trop long ({length} caractères, {} au plus) : garde seulement le fait utile, en une ou deux phrases", crate::mcp::MAX_MESSAGE);
                }
                let sender = caller.and_then(|c| self.tabs.iter().find(|t| t.id == c)).map(|t| format!("#{} « {} »", t.id, t.display_title())).unwrap_or_else(|| "un agent".into());
                let tab = self.tabs.iter().find(|t| t.id == id).ok_or_else(|| anyhow::anyhow!("aucune fenêtre #{id}"))?;
                if tab.terminal.read(cx).exited {
                    anyhow::bail!("l'agent #{id} est arrêté");
                }
                let reply = caller.map(|c| format!(", to={c} pour répondre si utile")).unwrap_or_default();
                audit::record(&tab.project.path, kind::CONVERSATION, format!("Message de l'agent {sender} à #{id}"), json!({ "message": message.chars().take(400).collect::<String>() }), None);
                let text = format!("[Message de l'agent {sender}{reply}] {message}");
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                    tab.inbox.push(text);
                }
                let delivered = self.deliver_messages(id, cx);
                cx.notify();
                Ok(vec![Content::Text(if delivered {
                    format!("Message remis à l'agent #{id}.")
                } else {
                    format!("Message transmis à l'agent #{id} : il le lira en direct dès sa prochaine action, sans attendre la fin de sa tâche. Inutile de le renvoyer.")
                })])
            }
            // Appelé par les hooks de l'agent (voir `mcp::agent_hooks`) : vide sa boîte.
            crate::mcp::INBOX_TOOL => {
                let tab = caller.and_then(|c| self.tabs.iter_mut().find(|t| t.id == c));
                let messages = tab.map(|t| std::mem::take(&mut t.inbox)).unwrap_or_default();
                if !messages.is_empty() {
                    cx.notify();
                }
                Ok(vec![Content::Text(messages.join("\n\n"))])
            }
            _ => anyhow::bail!("outil inconnu : {tool}"),
        }
    }
}
