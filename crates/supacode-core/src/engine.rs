//! Exécution d'un workflow : mêmes règles que le moteur TypeScript (conditions, `{{variables}}`,
//! variables `SUPACODE_*`, groupes parallèles, validations humaines).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use crate::agents::{self, AgentEvent};
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

pub struct RunOptions {
    pub cwd: PathBuf,
    /// Variables supplémentaires : `{{file}}`, `{{selection}}`…
    pub context: HashMap<String, String>,
    pub cancel: Arc<AtomicBool>,
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
    let state = Run { options, host, config, results: Mutex::new(Vec::new()), handled, groups, git: OnceLock::new() };

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
            (if approved { StepStatus::Success } else { StepStatus::Failed }, if approved { "validé" } else { "refusé" }.to_owned(), None)
        } else {
            let mut output = String::new();
            let result = if let Some(command) = &step.run {
                let mut spawn = Spawn::shell(command, &self.options.cwd);
                spawn.env = self.environment();
                process::run_lines(spawn, &self.options.cancel, |line| {
                    push_tail(&mut output, &line.text);
                    self.output(&id, AgentEvent::Log(line.text));
                })
            } else {
                let prompt = self.render(step.prompt.as_deref().unwrap_or_default());
                let forced = match step.agent.as_deref() {
                    Some("claude") => Some(AgentId::Claude),
                    Some("codex") => Some(AgentId::Codex),
                    _ => None,
                };
                let decision = router::route(&prompt, &self.config, forced, step.model.as_deref());
                self.host.emit(Event::Routed { id: id.clone(), agent: decision.agent, model: decision.model.clone(), reason: decision.reason });
                agents::run(decision.agent, decision.model.as_deref(), &prompt, &self.options.cwd, &self.options.cancel, |event| {
                    match &event {
                        AgentEvent::Text(text) => push_tail(&mut output, text),
                        AgentEvent::Done { summary: Some(summary) } => output = summary.clone(),
                        _ => {}
                    }
                    self.output(&id, event);
                })
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
        RunOptions { cwd: std::env::temp_dir(), context: HashMap::from([("nom".into(), "monde".into())]), cancel: Arc::default() }
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
