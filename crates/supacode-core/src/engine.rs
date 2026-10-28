//! Exécution d'un workflow : mêmes règles que le moteur TypeScript (conditions, `{{variables}}`,
//! variables `SUPACODE_*`, groupes parallèles, validations humaines).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use crate::agents::{self, AgentEvent};
use crate::audit::{self, kind};
use crate::policy::{self, Policy};
use crate::prompts;
use crate::process::{self, Spawn};
use crate::router::{self, AgentId, OrchestratorConfig};
use crate::workflows::{Step, Workflow};

/// Taille maximale conservée pour la sortie d'une étape (fin de sortie).
const MAX_OUTPUT: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Run,
    Agent,
    Approve,
    Parallel,
}

impl Step {
    pub fn kind(&self) -> StepKind {
        if self.parallel.is_some() {
            StepKind::Parallel
        } else if self.approve.is_some() {
            StepKind::Approve
        } else if self.run.is_some() {
            StepKind::Run
        } else {
            StepKind::Agent
        }
    }

    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Success,
    Failed,
    Skipped,
}

impl StepStatus {
    fn as_str(self) -> &'static str {
        match self {
            StepStatus::Success => "success",
            StepStatus::Failed => "failed",
            StepStatus::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Success,
    Failed,
    Cancelled,
}

/// Événements d'une exécution, dans l'ordre où ils se produisent.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    StepStart { id: String },
    /// Agent choisi par l'orchestrateur pour une étape `prompt`.
    Routed { id: String, agent: AgentId, model: Option<String>, reason: String },
    /// Sortie d'une commande, ou activité d'un agent.
    Output { id: String, event: AgentEvent },
    /// Une validation humaine est attendue : répondre via [`Host::approve`].
    Approval { id: String, message: String },
    StepDone { id: String, status: StepStatus, exit_code: Option<i32> },
    Done { status: RunStatus },
}

/// Ce que l'IDE (ou la CLI) fournit au moteur.
pub trait Host: Send + Sync {
    fn emit(&self, event: Event);
    /// Bloque jusqu'à la décision de l'utilisateur ; faux si refus ou arrêt.
    fn approve(&self, id: &str, message: &str, cancel: &AtomicBool) -> bool;
}

#[derive(Default)]
pub struct RunOptions {
    pub cwd: PathBuf,
    /// Variables supplémentaires : `{{file}}`, `{{selection}}`…
    pub context: HashMap<String, String>,
    pub cancel: Arc<AtomicBool>,
    /// Règles d'équipe du projet.
    pub policy: Option<Policy>,
    /// Déjà dépensé (estimation) aujourd'hui et ce mois, pour les budgets.
    pub spent: Spent,
    /// Tenir le journal d'audit du projet.
    pub journal: bool,
    /// Réglages ajoutés à `claude --settings` (skills et serveurs MCP désactivés).
    pub claude_settings: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Spent {
    pub today: f64,
    pub month: f64,
}

struct StepResult {
    status: StepStatus,
    output: String,
}

struct Run<'a> {
    options: &'a RunOptions,
    host: &'a dyn Host,
    config: OrchestratorConfig,
    results: Mutex<Vec<(String, StepResult)>>,
    /// Étapes dont l'échec est rattrapé par une étape `if: "steps.<id>.failed"`.
    handled: HashSet<String>,
    /// Groupes parallèles : leur échec est déjà compté via leurs étapes.
    groups: HashSet<String>,
    git: OnceLock<HashMap<String, String>>,
    /// Coût des étapes d'agent de cette exécution.
    cost: Mutex<f64>,
}

/// Toutes les étapes, groupes parallèles compris, à plat.
pub fn flatten(steps: &[Step]) -> Vec<&Step> {
    let mut all = Vec::new();
    for step in steps {
        all.push(step);
        if let Some(children) = &step.parallel {
            all.extend(flatten(children));
        }
    }
    all
}

/// Exécute le workflow jusqu'au bout (ou jusqu'à l'arrêt) ; bloquant.
pub fn run(workflow: &Workflow, options: &RunOptions, host: &dyn Host) -> RunStatus {
    let all = flatten(&workflow.steps);
    let mut handled = HashSet::new();
    for step in &all {
        let Some(id) = step.condition.as_deref().and_then(|c| c.strip_prefix("steps.")).and_then(|c| c.strip_suffix(".failed")) else { continue };
        if let Some(target) = all.iter().find(|s| s.id == id) {
            // Rattraper l'échec d'un groupe parallèle rattrape aussi celui de ses étapes.
            handled.extend(flatten(std::slice::from_ref(*target)).into_iter().map(|s| s.id.clone()));
        }
    }
    let groups = all.iter().filter(|s| s.parallel.is_some()).map(|s| s.id.clone()).collect();
    let config = router::load_config(&options.cwd).unwrap_or_default();
    let state = Run { options, host, config, results: Mutex::new(Vec::new()), handled, groups, git: OnceLock::new(), cost: Mutex::new(0.0) };
    state.journal(kind::WORKFLOW, format!("Workflow « {} » lancé", workflow.name), serde_json::json!({ "contexte": options.context }), None);

    for step in &workflow.steps {
        if state.cancelled() {
            break;
        }
        state.run_step(step);
    }
    let status = if state.cancelled() {
        RunStatus::Cancelled
    } else if state.unhandled_failure() {
        RunStatus::Failed
    } else {
        RunStatus::Success
    };
    let label = match status {
        RunStatus::Success => "réussi",
        RunStatus::Failed => "en échec",
        RunStatus::Cancelled => "arrêté",
    };
    let cost = *state.cost.lock().unwrap();
    state.journal(kind::WORKFLOW, format!("Workflow « {} » {label}", workflow.name), serde_json::Value::Null, (cost > 0.0).then_some(cost));
    host.emit(Event::Done { status });
    status
}

impl Run<'_> {
    fn cancelled(&self) -> bool {
        self.options.cancel.load(Ordering::Relaxed)
    }

    fn status_of(&self, id: &str) -> Option<StepStatus> {
        self.results.lock().unwrap().iter().find(|(i, _)| i == id).map(|(_, r)| r.status)
    }

    fn unhandled_failure(&self) -> bool {
        self.results
            .lock()
            .unwrap()
            .iter()
            .any(|(id, r)| r.status == StepStatus::Failed && !self.handled.contains(id) && !self.groups.contains(id))
    }

    fn should_run(&self, step: &Step) -> bool {
        match step.condition.as_deref() {
            Some("always") => true,
            Some(condition) => {
                let mut parts = condition.split('.');
                let (_, id, expected) = (parts.next(), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                self.status_of(id).is_some_and(|s| s.as_str() == expected)
            }
            None => !self.unhandled_failure(),
        }
    }

    fn journal(&self, kind: &str, summary: String, details: serde_json::Value, cost: Option<f64>) {
        if self.options.journal {
            audit::record(&self.options.cwd, kind, summary, details, cost);
        }
    }

    /// Refus des règles d'équipe : l'étape échoue avec ce message.
    fn refuse(&self, id: &str, message: String) -> (StepStatus, String, Option<i32>) {
        self.output(id, AgentEvent::Error(message.clone()));
        self.journal(kind::BLOCKED, message.clone(), serde_json::json!({ "étape": id }), None);
        (StepStatus::Failed, message, None)
    }

    /// Budget dépassé ? Message à afficher si oui.
    fn over_budget(&self) -> Option<String> {
        let budget = &self.options.policy.as_ref()?.budget;
        let run = *self.cost.lock().unwrap();
        let spent = self.options.spent;
        let over = |limit: Option<f64>, used: f64, label: &str| limit.filter(|l| used >= *l).map(|l| format!("Budget {label} atteint ({used:.2} $ sur {l:.2} $) : étape d'agent refusée par les règles d'équipe"));
        over(budget.per_run_usd, run, "de l'exécution")
            .or_else(|| over(budget.daily_usd, spent.today + run, "du jour"))
            .or_else(|| over(budget.monthly_usd, spent.month + run, "du mois"))
    }

    fn output(&self, id: &str, event: AgentEvent) {
        self.host.emit(Event::Output { id: id.to_owned(), event });
    }

    fn run_step(&self, step: &Step) {
        let id = step.id.clone();
        self.host.emit(Event::StepStart { id: id.clone() });
        if !self.should_run(step) {
            self.finish(&id, StepStatus::Skipped, String::new(), None);
            return;
        }

        let (status, output, exit_code) = if let Some(children) = &step.parallel {
            thread::scope(|scope| {
                for child in children {
                    scope.spawn(move || self.run_step(child));
                }
            });
            let failed = children.iter().any(|c| self.status_of(&c.id) == Some(StepStatus::Failed));
            (if failed { StepStatus::Failed } else { StepStatus::Success }, String::new(), None)
        } else if let Some(message) = &step.approve {
            let message = self.render(message);
            self.host.emit(Event::Approval { id: id.clone(), message: message.clone() });
            let approved = self.host.approve(&id, &message, &self.options.cancel);
            self.journal(kind::APPROVAL, format!("{} : {message}", if approved { "Validé" } else { "Refusé" }), serde_json::json!({ "étape": id }), None);
            (if approved { StepStatus::Success } else { StepStatus::Failed }, if approved { "validé" } else { "refusé" }.to_owned(), None)
        } else {
            let mut output = String::new();
            let policy = self.options.policy.as_ref();
            let blocked = step.run.as_deref().and_then(|command| policy.and_then(|p| p.blocked_command(command)).map(|pattern| (command, pattern)));
            let result = if let Some((command, pattern)) = blocked {
                let (status, message, code) = self.refuse(&id, format!("Commande bloquée par les règles d'équipe (« {pattern} ») : {command}"));
                self.finish(&id, status, message, code);
                return;
            } else if let Some(command) = &step.run {
                let mut spawn = Spawn::shell(command, &self.options.cwd);
                spawn.env = self.environment();
                let result = process::run_lines(spawn, &self.options.cancel, |line| {
                    push_tail(&mut output, &line.text);
                    self.output(&id, AgentEvent::Log(line.text));
                });
                let code = result.as_ref().ok().copied().flatten();
                self.journal(kind::COMMAND, command.clone(), serde_json::json!({ "étape": id, "code": code }), None);
                if step.expect.as_deref() == Some("fail") {
                    // Le test doit reproduire le bug : un échec est le résultat attendu.
                    result.map(|code| match code {
                        Some(0) => {
                            self.output(&id, AgentEvent::Error("Cette commande devait échouer : le test passe déjà, il ne reproduit pas le bug.".into()));
                            Some(1)
                        }
                        Some(_) => {
                            self.output(&id, AgentEvent::Log("✓ échec attendu : le test reproduit le bug.".into()));
                            Some(0)
                        }
                        None => None,
                    })
                } else {
                    result
                }
            } else {
                let raw = step.prompt.as_deref().unwrap_or_default();
                let mut from_library = None;
                let raw = match raw.trim().strip_prefix("@prompts/") {
                    Some(name) => match prompts::load(&self.options.cwd, name) {
                        Ok(file) => {
                            from_library = Some((file.id.clone(), prompts::version(&self.options.cwd, &file.id)));
                            if let Some((name, version)) = &from_library {
                                self.output(&id, AgentEvent::Log(format!("prompt « {name} », version {version}")));
                            }
                            file.body
                        }
                        Err(e) => {
                            let (status, message, code) = self.refuse(&id, format!("{e:#}"));
                            self.finish(&id, status, message, code);
                            return;
                        }
                    },
                    None => raw.to_owned(),
                };
                let prompt = self.render(&raw);
                let forced = match step.agent.as_deref() {
                    Some("claude") => Some(AgentId::Claude),
                    Some("codex") => Some(AgentId::Codex),
                    _ => None,
                };
                let mut decision = router::route(&prompt, &self.config, forced, step.model.as_deref());
                if let Some(Err(message)) = policy.map(|p| p.constrain(&mut decision)) {
                    let (status, message, code) = self.refuse(&id, format!("Étape refusée : {message}"));
                    self.finish(&id, status, message, code);
                    return;
                }
                if let Some(message) = self.over_budget() {
                    let (status, message, code) = self.refuse(&id, message);
                    self.finish(&id, status, message, code);
                    return;
                }
                let (agent, model) = (decision.agent, decision.model.clone());
                self.host.emit(Event::Routed { id: id.clone(), agent, model: model.clone(), reason: decision.reason });
                // Claude applique lui-même les interdictions ; pour Codex, on vérifie après coup.
                let from_policy = policy.and_then(Policy::claude_settings).and_then(|s| serde_json::from_str(&s).ok());
                let extra: Vec<String> = match (agent, crate::extensions::merge_settings(from_policy, self.options.claude_settings.clone())) {
                    (AgentId::Claude, Some(settings)) => vec!["--settings".into(), settings.to_string()],
                    _ => Vec::new(),
                };
                let (mut tools, mut cost, mut violation) = (Vec::new(), None, None::<String>);
                let result = agents::run(agent, model.as_deref(), &prompt, &self.options.cwd, &extra, &self.options.cancel, |event| {
                    match &event {
                        AgentEvent::Text(text) => push_tail(&mut output, text),
                        AgentEvent::Done { summary: Some(summary) } => output = summary.clone(),
                        AgentEvent::Usage { cost_usd } => cost = Some(*cost_usd),
                        AgentEvent::Tool { name, detail } => {
                            let detail = detail.clone().unwrap_or_default();
                            if let Some(p) = policy.filter(|_| agent == AgentId::Codex) {
                                let broken = if name == "shell" {
                                    p.blocked_command(&detail).map(|pattern| format!("commande « {detail} » (règle « {pattern} »)"))
                                } else {
                                    detail.split(", ").find_map(|file| {
                                        // Codex donne des chemins absolus : on les ramène au projet.
                                        let relative = Path::new(file).strip_prefix(&self.options.cwd).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| file.to_owned());
                                        p.protected_file(&relative).map(|pattern| format!("fichier protégé « {file} » (règle « {pattern} »)"))
                                    })
                                };
                                if let Some(broken) = broken {
                                    violation.get_or_insert(broken);
                                }
                            }
                            tools.push(serde_json::json!({ "outil": name, "détail": detail }));
                        }
                        _ => {}
                    }
                    self.output(&id, event);
                });
                if let Some(cost) = cost {
                    *self.cost.lock().unwrap() += cost;
                }
                let model_label = model.clone().unwrap_or_else(|| "modèle par défaut".into());
                self.journal(
                    kind::AGENT,
                    format!("{} ({model_label}) : {}", agent.label(), prompt.chars().take(160).collect::<String>()),
                    serde_json::json!({
                        "étape": id,
                        "agent": agent.as_str(),
                        "modèle": model,
                        "outils": tools,
                        "prompt": from_library.as_ref().map(|(name, _)| name),
                        "version": from_library.as_ref().map(|(_, version)| version),
                    }),
                    cost,
                );
                match violation {
                    Some(broken) => {
                        let (status, message, code) = self.refuse(&id, format!("Codex a enfreint les règles d'équipe : {broken}"));
                        self.finish(&id, status, message, code);
                        return;
                    }
                    None => {
                        let limit = policy.and_then(|p| p.max_diff_lines);
                        match limit.and_then(|limit| policy::changed_lines(&self.options.cwd).filter(|n| *n > limit).map(|n| (n, limit))) {
                            Some((lines, limit)) if result.as_ref().is_ok_and(|c| *c == Some(0)) => {
                                let message = format!("Lot trop gros pour être relu : {lines} lignes modifiées (limite des règles d'équipe : {limit}). Découpe le travail avant de continuer.");
                                let (status, message, code) = self.refuse(&id, message);
                                self.finish(&id, status, message, code);
                                return;
                            }
                            _ => result,
                        }
                    }
                }
            };
            match result {
                Ok(code) => (if code == Some(0) { StepStatus::Success } else { StepStatus::Failed }, output, code),
                Err(err) => {
                    self.output(&id, AgentEvent::Error(format!("{err:#}")));
                    (StepStatus::Failed, output, None)
                }
            }
        };
        self.finish(&id, status, output.trim_end().to_owned(), exit_code);
    }

    fn finish(&self, id: &str, status: StepStatus, output: String, exit_code: Option<i32>) {
        self.results.lock().unwrap().push((id.to_owned(), StepResult { status, output }));
        self.host.emit(Event::StepDone { id: id.to_owned(), status, exit_code });
    }

    /// `{{git.diff}}`, `{{git.branch}}`, `{{steps.<id>.output|status}}` et le contexte.
    fn variables(&self, with_git: bool) -> HashMap<String, String> {
        let mut vars = self.options.context.clone();
        for (id, result) in self.results.lock().unwrap().iter() {
            vars.insert(format!("steps.{id}.output"), result.output.clone());
            vars.insert(format!("steps.{id}.status"), result.status.as_str().to_owned());
        }
        if with_git {
            vars.extend(self.git().clone());
        }
        vars
    }

    fn git(&self) -> &HashMap<String, String> {
        self.git.get_or_init(|| {
            let git = |args: &[&str]| {
                let mut out = String::new();
                let spawn = Spawn::program("git", args.iter().map(|a| (*a).to_owned()).collect(), &self.options.cwd);
                let _ = process::run_lines(spawn, &AtomicBool::new(false), |line| {
                    if !line.stderr {
                        out.push_str(&line.text);
                        out.push('\n');
                    }
                });
                out.trim_end().to_owned()
            };
            let staged = git(&["diff", "--cached"]);
            let diff = if staged.is_empty() { git(&["diff"]) } else { staged };
            HashMap::from([("git.diff".to_owned(), diff), ("git.branch".to_owned(), git(&["rev-parse", "--abbrev-ref", "HEAD"]))])
        })
    }

    fn render(&self, template: &str) -> String {
        render(template, &self.variables(template.contains("{{git.")))
    }

    /// Contexte exposé aux commandes : SUPACODE_FILE, SUPACODE_STEP_LINT_OUTPUT…
    fn environment(&self) -> HashMap<String, String> {
        self.variables(false)
            .into_iter()
            .map(|(key, value)| {
                let key = key.strip_prefix("steps.").map(|k| format!("step.{k}")).unwrap_or(key);
                let key: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c.to_ascii_uppercase() } else { '_' }).collect();
                (format!("SUPACODE_{key}"), value)
            })
            .collect()
    }
}

fn push_tail(output: &mut String, line: &str) {
    output.push_str(line);
    output.push('\n');
    if output.len() > MAX_OUTPUT {
        let mut cut = output.len() - MAX_OUTPUT;
        while !output.is_char_boundary(cut) {
            cut += 1;
        }
        output.drain(..cut);
    }
}

/// Remplace les `{{nom}}` connus ; les inconnus sont laissés tels quels.
pub fn render(template: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = after[..end].trim();
                let valid = !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-'));
                match vars.get(key).filter(|_| valid) {
                    Some(value) => out.push_str(value),
                    None => out.push_str(&rest[start..start + 2 + end + 2]),
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recorder {
        events: Mutex<Vec<Event>>,
        answer: bool,
    }

    impl Host for Recorder {
        fn emit(&self, event: Event) {
            self.events.lock().unwrap().push(event);
        }
        fn approve(&self, _: &str, _: &str, _: &AtomicBool) -> bool {
            self.answer
        }
    }

    fn workflow(json: &str) -> Workflow {
        serde_json::from_str(json).unwrap()
    }

    fn statuses(host: &Recorder) -> Vec<(String, StepStatus)> {
        host.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::StepDone { id, status, .. } => Some((id.clone(), *status)),
                _ => None,
            })
            .collect()
    }

    fn options() -> RunOptions {
        RunOptions { cwd: std::env::temp_dir(), context: HashMap::from([("nom".into(), "monde".into())]), ..Default::default() }
    }

    #[test]
    fn rattrape_un_echec_et_passe_la_sortie() {
        let wf = workflow(
            r#"{"name":"t","steps":[
                {"id":"a","run":"echo bonjour"},
                {"id":"b","run":"exit 2"},
                {"id":"c","run":"echo aussi"},
                {"id":"rattrapage","if":"steps.b.failed","approve":"{{steps.a.output}} {{nom}} {{inconnu}}"}
            ]}"#,
        );
        let host = Recorder { events: Mutex::default(), answer: true };
        let status = run(&wf, &options(), &host);
        assert_eq!(status, RunStatus::Success);
        let s = statuses(&host);
        assert_eq!(s, [("a".into(), StepStatus::Success), ("b".into(), StepStatus::Failed), ("c".into(), StepStatus::Success), ("rattrapage".into(), StepStatus::Success)]);
        let events = host.events.lock().unwrap();
        let message = events.iter().find_map(|e| match e {
            Event::Approval { message, .. } => Some(message.clone()),
            _ => None,
        });
        assert_eq!(message.as_deref().map(str::trim), Some("bonjour monde {{inconnu}}"));
    }

    #[test]
    fn commande_qui_doit_echouer() {
        let wf = workflow(
            r#"{"name":"t","steps":[
                {"id":"rouge","run":"exit 3","expect":"fail"},
                {"id":"deja_vert","run":"echo ok","expect":"fail"},
                {"id":"apres","run":"echo apres"}
            ]}"#,
        );
        let host = Recorder { events: Mutex::default(), answer: true };
        assert_eq!(run(&wf, &options(), &host), RunStatus::Failed);
        assert_eq!(statuses(&host), [("rouge".into(), StepStatus::Success), ("deja_vert".into(), StepStatus::Failed), ("apres".into(), StepStatus::Skipped)]);
    }

    #[test]
    fn prompt_introuvable() {
        let wf = workflow(r#"{"name":"t","steps":[{"id":"a","prompt":"@prompts/inexistant"}]}"#);
        let host = Recorder { events: Mutex::default(), answer: true };
        assert_eq!(run(&wf, &options(), &host), RunStatus::Failed);
        let events = host.events.lock().unwrap();
        assert!(events.iter().any(|e| matches!(e, Event::Output { event: AgentEvent::Error(m), .. } if m.contains("introuvable"))));
    }

    #[test]
    fn regles_d_equipe() {
        let root = std::env::temp_dir().join(format!("supacode-regles-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let wf = workflow(
            r#"{"name":"t","steps":[
                {"id":"pousse","run":"echo ok && git push --force"},
                {"id":"apres","run":"echo apres"},
                {"id":"agent","if":"always","prompt":"Corrige"}
            ]}"#,
        );
        let policy = crate::policy::Policy {
            blocked_commands: vec!["git push --force*".into()],
            budget: crate::policy::Budget { daily_usd: Some(10.0), ..Default::default() },
            ..Default::default()
        };
        let options = RunOptions { cwd: root.clone(), policy: Some(policy), spent: Spent { today: 12.0, month: 12.0 }, journal: true, ..Default::default() };
        let host = Recorder { events: Mutex::default(), answer: true };
        assert_eq!(run(&wf, &options, &host), RunStatus::Failed);
        // Bloquée avant de tourner, la suite est sautée, et le budget du jour refuse l'agent sans le lancer.
        assert_eq!(statuses(&host), [("pousse".into(), StepStatus::Failed), ("apres".into(), StepStatus::Skipped), ("agent".into(), StepStatus::Failed)]);
        let journal = audit::read(&root);
        let blocked: Vec<_> = journal.iter().filter(|e| e.kind == kind::BLOCKED).map(|e| e.summary.clone()).collect();
        assert_eq!(blocked.len(), 2, "{blocked:?}");
        assert!(blocked[1].contains("Budget du jour"));
        assert!(journal.iter().all(|e| e.kind != kind::COMMAND), "la commande bloquée n'a pas tourné");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn groupe_parallele_et_refus() {
        let wf = workflow(
            r#"{"name":"t","steps":[
                {"id":"g","parallel":[{"id":"x","run":"echo x"},{"id":"y","run":"echo y"}]},
                {"id":"ok","approve":"Valider ?"},
                {"id":"apres","run":"echo apres"}
            ]}"#,
        );
        let host = Recorder { events: Mutex::default(), answer: false };
        assert_eq!(run(&wf, &options(), &host), RunStatus::Failed);
        let s = statuses(&host);
        assert!(s.contains(&("g".into(), StepStatus::Success)));
        assert!(s.contains(&("ok".into(), StepStatus::Failed)));
        assert!(s.contains(&("apres".into(), StepStatus::Skipped)));
    }

    #[test]
    fn variables_d_environnement() {
        let wf = workflow(
            if cfg!(windows) {
                r#"{"name":"t","steps":[{"id":"a","run":"echo salut"},{"id":"b","run":"echo %SUPACODE_STEP_A_OUTPUT%-%SUPACODE_NOM%"}]}"#
            } else {
                r#"{"name":"t","steps":[{"id":"a","run":"echo salut"},{"id":"b","run":"echo $SUPACODE_STEP_A_OUTPUT-$SUPACODE_NOM"}]}"#
            },
        );
        let host = Recorder { events: Mutex::default(), answer: true };
        run(&wf, &options(), &host);
        let events = host.events.lock().unwrap();
        let line = events.iter().rev().find_map(|e| match e {
            Event::Output { id, event: AgentEvent::Log(l) } if id == "b" => Some(l.trim().to_owned()),
            _ => None,
        });
        assert_eq!(line.as_deref(), Some("salut-monde"));
    }
}
