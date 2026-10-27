//! Agents en mode non interactif : `claude -p` (stream-json) et `codex exec --json`.
//! La demande passe par l'entrée standard : aucun souci d'échappement avec `cmd.exe`.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use serde_json::Value;

use crate::process::{self, Spawn};
use crate::router::AgentId;

/// Ce que l'agent fait, au fil de l'eau.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// Message de l'agent.
    Text(String),
    /// Outil utilisé : nom et détail court (fichier, commande…).
    Tool { name: String, detail: Option<String> },
    /// Sortie brute (stderr, lignes non reconnues).
    Log(String),
    Error(String),
    /// Résultat final.
    Done { summary: Option<String> },
}

/// Lance l'agent sur `prompt` dans `cwd` ; renvoie le code de sortie (`None` si arrêté).
pub fn run(agent: AgentId, model: Option<&str>, prompt: &str, cwd: &Path, cancel: &AtomicBool, mut on_event: impl FnMut(AgentEvent)) -> Result<Option<i32>> {
    let mut args: Vec<String> = match agent {
        // `--no-session-persistence` : les étapes de workflow n'encombrent pas la liste des conversations.
        AgentId::Claude => ["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", "acceptEdits", "--no-session-persistence"].map(Into::into).into(),
        // `--sandbox workspace-write` : équivalent de l'ancien `--full-auto` (retiré des versions récentes).
        AgentId::Codex => ["exec", "--json", "--sandbox", "workspace-write"].map(Into::into).into(),
    };
    if let Some(model) = model {
        args.extend(["--model".into(), model.into()]);
    }
    if agent == AgentId::Codex {
        // « - » : la demande est lue sur l'entrée standard.
        args.push("-".into());
    }
    let program = match agent {
        AgentId::Claude => "claude",
        AgentId::Codex => "codex",
    };
    let mut spawn = Spawn::program(program, args, cwd);
    spawn.stdin = Some(prompt.to_owned());
    let mut last_error = None::<String>;
    let code = process::run_lines(spawn, cancel, |line| {
        let events = if line.stderr {
            vec![AgentEvent::Log(line.text)]
        } else {
            match agent {
                AgentId::Claude => parse_claude_line(&line.text),
                AgentId::Codex => parse_codex_line(&line.text),
            }
        };
        for event in events {
            if let AgentEvent::Error(message) = &event {
                // Codex répète parfois la même erreur : on ne l'affiche qu'une fois.
                if last_error.as_deref() == Some(message) {
                    continue;
                }
                last_error = Some(message.clone());
            }
            on_event(event);
        }
    })?;
    // Une erreur signalée par l'agent fait échouer l'étape, même avec un code 0.
    Ok(code.map(|c| if c == 0 && last_error.is_some() { 1 } else { c }))
}

fn parse_object(line: &str) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_str(line) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

fn str_field<'a>(map: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

/// Résumé court de l'entrée d'un outil.
fn tool_detail(input: Option<&Value>) -> Option<String> {
    let fields = input?.as_object()?;
    ["file_path", "path", "command", "pattern", "url", "description"]
        .iter()
        .find_map(|key| fields.get(*key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned))
}

/// Ligne `stream-json` de Claude Code → événements.
pub fn parse_claude_line(line: &str) -> Vec<AgentEvent> {
    let Some(msg) = parse_object(line) else {
        return if line.trim().is_empty() { vec![] } else { vec![AgentEvent::Log(line.to_owned())] };
    };
    match str_field(&msg, "type") {
        Some("assistant") => {
            let content = msg.get("message").and_then(|m| m.get("content")).and_then(Value::as_array);
            content
                .into_iter()
                .flatten()
                .filter_map(|block| match block.get("type").and_then(Value::as_str) {
                    Some("text") => block.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty()).map(|t| AgentEvent::Text(t.to_owned())),
                    Some("tool_use") => block
                        .get("name")
                        .and_then(Value::as_str)
                        .map(|name| AgentEvent::Tool { name: name.to_owned(), detail: tool_detail(block.get("input")) }),
                    _ => None,
                })
                .collect()
        }
        Some("result") => {
            let result = str_field(&msg, "result").map(str::to_owned);
            let subtype = str_field(&msg, "subtype");
            if msg.get("is_error").and_then(Value::as_bool) == Some(true) || subtype.is_some_and(|s| s != "success") {
                vec![AgentEvent::Error(result.unwrap_or_else(|| format!("Claude a échoué ({})", subtype.unwrap_or("?"))))]
            } else {
                vec![AgentEvent::Done { summary: result }]
            }
        }
        _ => vec![],
    }
}

/// Ligne JSONL de `codex exec --json` → événements.
pub fn parse_codex_line(line: &str) -> Vec<AgentEvent> {
    let Some(msg) = parse_object(line) else {
        return if line.trim().is_empty() { vec![] } else { vec![AgentEvent::Log(line.to_owned())] };
    };
    match str_field(&msg, "type") {
        Some("item.completed") => {
            let Some(item) = msg.get("item").and_then(Value::as_object) else { return vec![] };
            match str_field(item, "type") {
                Some("agent_message") => str_field(item, "text").map(|t| AgentEvent::Text(t.to_owned())).into_iter().collect(),
                Some("command_execution") => vec![AgentEvent::Tool { name: "shell".into(), detail: str_field(item, "command").map(str::to_owned) }],
                Some("file_change") => {
                    let paths: Vec<_> = item
                        .get("changes")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c.get("path").and_then(Value::as_str))
                        .collect();
                    vec![AgentEvent::Tool { name: "edit".into(), detail: (!paths.is_empty()).then(|| paths.join(", ")) }]
                }
                _ => vec![],
            }
        }
        Some("error") | Some("turn.failed") => {
            let message = str_field(&msg, "message")
                .or_else(|| msg.get("error").and_then(|e| e.get("message")).and_then(Value::as_str))
                .unwrap_or("Codex a échoué");
            // Codex relaie parfois l'erreur brute de l'API sous forme de JSON : on en extrait le message.
            let api = parse_object(message).and_then(|m| m.get("error").and_then(|e| e.get("message")).and_then(Value::as_str).map(str::to_owned));
            vec![AgentEvent::Error(api.unwrap_or_else(|| message.to_owned()))]
        }
        Some("turn.completed") => vec![AgentEvent::Done { summary: None }],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lit_le_flux_de_claude() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Je lis."},{"type":"tool_use","name":"Read","input":{"file_path":"src/a.ts"}}]}}"#;
        assert_eq!(
            parse_claude_line(line),
            [AgentEvent::Text("Je lis.".into()), AgentEvent::Tool { name: "Read".into(), detail: Some("src/a.ts".into()) }]
        );
        assert_eq!(parse_claude_line(r#"{"type":"result","subtype":"success","result":"Fait."}"#), [AgentEvent::Done { summary: Some("Fait.".into()) }]);
    }

    #[test]
    fn extrait_l_erreur_api_de_codex() {
        let line = r#"{"type":"error","message":"{\"error\":{\"message\":\"Quota dépassé\"}}"}"#;
        assert_eq!(parse_codex_line(line), [AgentEvent::Error("Quota dépassé".into())]);
    }
}
