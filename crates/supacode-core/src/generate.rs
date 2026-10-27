//! Création d'un workflow à partir d'une description en langage naturel : Claude lit le projet
//! (outils de lecture seulement) et répond avec le JSON du workflow, vérifié avant l'aperçu.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::agents::{AgentEvent, parse_claude_line};
use crate::engine::{StepKind, flatten};
use crate::process::{self, Spawn};
use crate::workflows::{Step, Trigger, Workflow};

/// Workflow proposé, avec les interprétations à signaler à l'utilisateur.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Generated {
    pub workflow: Workflow,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Instructions envoyées à Claude : format exact, et uniquement ce que le moteur sait exécuter.
pub fn prompt(description: &str) -> String {
    format!(
        r#"Tu crées un workflow SupaCode pour le projet du dossier courant.
Lis d'abord les fichiers utiles (package.json, Cargo.toml, Makefile, pyproject.toml…) pour utiliser les vraies commandes du projet. Ne modifie aucun fichier.

Réponds UNIQUEMENT avec un objet JSON, sans texte autour ni bloc de code :
{{"workflow": <workflow>, "notes": ["..."]}}

Format de <workflow> :
{{
  "name": "Nom court en français",
  "description": "Une phrase",
  "on": ["manual" | "commit" | "push" | {{"save": "<glob>"}}],
  "steps": [<étape>, ...]
}}
Chaque étape a un "id" unique (minuscules, chiffres, tirets), un "name" court en français, et exactement UN de ces champs :
- "run": "<commande shell>" pour une commande (lint, tests, build…) ;
- "prompt": "<consigne>" pour un agent, avec "agent": "auto" | "claude" | "codex" et, si utile, "model": "haiku" | "sonnet" | "opus" (Claude seulement) ;
- "approve": "<question>" pour une validation humaine ;
- "parallel": [<étapes>] pour lancer des étapes en même temps.
Condition facultative "if": "always" ou "steps.<id>.success" / "steps.<id>.failed" / "steps.<id>.skipped" (l'id doit désigner une étape PRÉCÉDENTE). Sans condition, une étape ne s'exécute que si aucune étape précédente n'a échoué ; une étape "if": "steps.X.failed" rattrape l'échec de X.
Variables utilisables dans "prompt" et "approve" : {{{{git.diff}}}}, {{{{git.branch}}}}, {{{{steps.<id>.output}}}}, {{{{steps.<id>.status}}}}, {{{{file}}}} (déclencheur save).
Pas de boucle ni de nouvelle tentative : si la demande en contient, fais au mieux (par exemple corriger puis relancer une fois) et explique-le dans "notes".
"notes" : en français, une phrase par interprétation ou limite (commande choisie, élément non pris en charge). Liste vide s'il n'y a rien à signaler.

Demande de l'utilisateur :
{description}"#
    )
}

/// Extrait et vérifie le JSON de la réponse de Claude.
pub fn parse(text: &str) -> Result<Generated> {
    let start = text.find('{').context("la réponse ne contient pas de JSON")?;
    let end = text.rfind('}').context("la réponse ne contient pas de JSON")?;
    let generated: Generated = serde_json::from_str(&text[start..=end]).context("le JSON proposé ne correspond pas au format des workflows")?;
    validate(&generated.workflow)?;
    Ok(generated)
}

/// Contrôles que la désérialisation ne fait pas : ids, type unique par étape, conditions.
pub fn validate(workflow: &Workflow) -> Result<()> {
    if workflow.name.trim().is_empty() {
        bail!("le workflow n'a pas de nom");
    }
    if workflow.steps.is_empty() {
        bail!("le workflow n'a aucune étape");
    }
    for trigger in workflow.triggers() {
        if let Trigger::Named(name) = &trigger {
            if !matches!(name.as_str(), "manual" | "commit" | "push") {
                bail!("déclencheur inconnu : « {name} »");
            }
        }
    }
    let mut seen = HashSet::new();
    for step in flatten(&workflow.steps) {
        if step.id.is_empty() || !step.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            bail!("id d'étape invalide : « {} »", step.id);
        }
        let kinds = [step.run.is_some(), step.prompt.is_some(), step.approve.is_some(), step.parallel.is_some()].iter().filter(|k| **k).count();
        if kinds != 1 {
            bail!("l'étape « {} » doit avoir exactement un des champs run, prompt, approve ou parallel", step.id);
        }
        if step.kind() == StepKind::Parallel && step.parallel.as_ref().is_some_and(Vec::is_empty) {
            bail!("le groupe parallèle « {} » est vide", step.id);
        }
        if let Some(condition) = step.condition.as_deref().filter(|c| *c != "always") {
            let target = condition.strip_prefix("steps.").and_then(|c| c.rsplit_once('.'));
            match target {
                Some((id, "success" | "failed" | "skipped")) if seen.contains(id) => {}
                _ => bail!("condition invalide pour « {} » : « {condition} »", step.id),
            }
        }
        if !seen.insert(step.id.clone()) {
            bail!("id d'étape en double : « {} »", step.id);
        }
    }
    Ok(())
}

/// Demande le workflow à Claude ; `on_event` reçoit son activité (fichiers lus…).
pub fn generate(description: &str, cwd: &Path, cancel: &AtomicBool, mut on_event: impl FnMut(AgentEvent)) -> Result<Generated> {
    let args = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--model",
        "sonnet",
        // Lecture seule : Claude peut regarder le projet, pas le modifier.
        "--tools",
        "Read,Glob,Grep",
    ];
    let mut spawn = Spawn::program("claude", args.map(Into::into).into(), cwd);
    spawn.stdin = Some(prompt(description));
    let (mut result, mut error) = (None, None);
    let code = process::run_lines(spawn, cancel, |line| {
        if line.stderr {
            return;
        }
        for event in parse_claude_line(&line.text) {
            match &event {
                AgentEvent::Done { summary } => result = summary.clone(),
                AgentEvent::Error(message) => error = Some(message.clone()),
                _ => {}
            }
            on_event(event);
        }
    })?;
    if let Some(error) = error {
        bail!("{error}");
    }
    match (code, result) {
        (None, _) => bail!("génération arrêtée"),
        (_, Some(text)) => parse(&text),
        (Some(code), None) => bail!("Claude s'est arrêté sans réponse (code {code})"),
    }
}

/// Identifiant de fichier à partir du nom : « Avant commit » → « avant-commit ».
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        let c = match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() { "workflow".into() } else { out }
}

/// Nombre de blocs affichés dans l'aperçu (étapes, groupes compris).
pub fn block_count(steps: &[Step]) -> usize {
    flatten(steps).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepte_une_reponse_valide_entouree_de_texte() {
        let text = r#"Voici : {"workflow":{"name":"Avant commit","on":["commit"],"steps":[
            {"id":"lint","name":"Lint","run":"npm run lint"},
            {"id":"g","parallel":[{"id":"tests","run":"npm test"},{"id":"types","run":"tsc --noEmit"}]},
            {"id":"fix","if":"steps.g.failed","agent":"codex","prompt":"Corrige :\n{{steps.tests.output}}"},
            {"id":"ok","approve":"Valider ?"}
        ]},"notes":["« types » → tsc --noEmit"]}"#;
        let g = parse(text).unwrap();
        assert_eq!(g.workflow.name, "Avant commit");
        assert_eq!(block_count(&g.workflow.steps), 6);
        assert_eq!(g.notes.len(), 1);
    }

    #[test]
    fn refuse_les_workflows_incoherents() {
        let bad = |steps: &str| parse(&format!(r#"{{"workflow":{{"name":"x","steps":{steps}}}}}"#)).unwrap_err().to_string();
        assert!(bad(r#"[{"id":"a","run":"x","approve":"y"}]"#).contains("exactement un"));
        assert!(bad(r#"[{"id":"a","run":"x"},{"id":"a","run":"y"}]"#).contains("double"));
        assert!(bad(r#"[{"id":"a","if":"steps.b.failed","run":"x"},{"id":"b","run":"y"}]"#).contains("condition"));
        assert!(parse("pas de json").is_err());
    }

    #[test]
    fn slug_lisible() {
        assert_eq!(slug("Avant commit"), "avant-commit");
        assert_eq!(slug("Review élégante !"), "review-elegante");
        assert_eq!(slug("!!!"), "workflow");
    }
}
