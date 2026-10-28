//! Plugins de Claude Code (skills, commandes, agents et serveurs MCP réunis), gérés par
//! la CLI : `claude plugin list --available --json`, `install`, `enable`, `disable`,
//! `uninstall`. Contrairement aux désactivations des extensions, ces réglages valent pour
//! tout Claude Code, comme le menu `/plugin`.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use crate::extensions::Scope;
use crate::process::{self, Spawn};

#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    /// `nom@marketplace`.
    pub id: String,
    pub version: String,
    /// `user`, `project` ou `local`.
    pub scope: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Available {
    pub id: String,
    pub name: String,
    pub description: String,
    pub marketplace: String,
    pub installs: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    pub installed: Vec<Installed>,
    /// Du plus installé au moins installé.
    pub available: Vec<Available>,
}

/// Commande déclarée par une marketplace, à faire accepter par une personne.
#[derive(Debug, Clone, PartialEq)]
pub struct Confirmation {
    pub command: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done(String),
    /// L'installation lancerait une commande : on la montre avant de l'accepter.
    NeedsConfirmation(Confirmation),
}

fn claude(args: &[String], cwd: &Path) -> Result<(Option<i32>, Vec<String>)> {
    let mut lines = Vec::new();
    let spawn = Spawn::program("claude", args.to_vec(), cwd);
    let code = process::run_lines(spawn, &AtomicBool::new(false), |line| {
        if !line.stderr {
            lines.push(line.text);
        }
    })?;
    Ok((code, lines))
}

pub fn catalog(cwd: &Path) -> Result<Catalog> {
    let (_, lines) = claude(&["plugin".into(), "list".into(), "--available".into(), "--json".into()], cwd)?;
    let value: Value = serde_json::from_str(&lines.join("\n")).map_err(|_| anyhow!("réponse inattendue de « claude plugin list »"))?;
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let installed = value.get("installed").and_then(Value::as_array).into_iter().flatten().map(|p| Installed {
        id: text(p, "id"),
        version: text(p, "version"),
        scope: text(p, "scope"),
        enabled: p.get("enabled").and_then(Value::as_bool).unwrap_or(true),
    });
    let mut available: Vec<Available> = value
        .get("available")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|p| Available {
            id: text(p, "pluginId"),
            name: text(p, "name"),
            description: text(p, "description"),
            marketplace: text(p, "marketplaceName"),
            installs: p.get("installCount").and_then(Value::as_u64),
        })
        .collect();
    // Les plus installés d'abord ; le répertoire officiel avant les autres à égalité.
    available.sort_by(|a, b| b.installs.unwrap_or(0).cmp(&a.installs.unwrap_or(0)).then((a.marketplace != "claude-plugins-official").cmp(&(b.marketplace != "claude-plugins-official"))).then(a.name.cmp(&b.name)));
    Ok(Catalog { installed: installed.collect(), available })
}

fn scope_arg(scope: Scope) -> &'static str {
    match scope {
        Scope::User => "user",
        Scope::Project => "project",
    }
}

/// Dernière ligne JSON d'une commande `--json` → résultat.
fn outcome(code: Option<i32>, lines: &[String]) -> Result<Outcome> {
    let value: Value = lines.iter().rev().find_map(|l| serde_json::from_str(l).ok()).ok_or_else(|| anyhow!("pas de réponse de la CLI claude"))?;
    if let Some(shown) = value.get("shownCommand") {
        let command = shown.get("command").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| shown.to_string());
        if let Some(sha256) = shown.get("sha256").and_then(Value::as_str) {
            if value.get("outcome").and_then(Value::as_str) != Some("success") {
                return Ok(Outcome::NeedsConfirmation(Confirmation { command, sha256: sha256.to_owned() }));
            }
        }
    }
    let message = value.get("message").and_then(Value::as_str).unwrap_or("").to_owned();
    match value.get("outcome").and_then(Value::as_str) {
        Some("failed") | Some("error") => bail!("{}", if message.is_empty() { "échec de la CLI claude".into() } else { message }),
        _ if code.is_some_and(|c| c != 0) => bail!("{}", if message.is_empty() { format!("la CLI claude a échoué (code {})", code.unwrap_or(-1)) } else { message }),
        _ => Ok(Outcome::Done(message)),
    }
}

/// Installe un plugin. Sans `accept`, une commande déclarée par la marketplace n'est pas
/// lancée : elle revient en `NeedsConfirmation` pour être montrée à l'utilisateur.
pub fn install(id: &str, scope: Scope, cwd: &Path, accept: Option<&str>) -> Result<Outcome> {
    let mut args: Vec<String> = vec!["plugin".into(), "install".into(), id.into(), "--scope".into(), scope_arg(scope).into(), "--json".into()];
    if let Some(sha) = accept {
        args.extend(["--accept-command".into(), sha.into()]);
    }
    let (code, lines) = claude(&args, cwd)?;
    outcome(code, &lines)
}

pub fn set_enabled(id: &str, scope: &str, enabled: bool, cwd: &Path) -> Result<()> {
    let action = if enabled { "enable" } else { "disable" };
    let (code, lines) = claude(&["plugin".into(), action.into(), id.into(), "--scope".into(), scope.into(), "--json".into()], cwd)?;
    outcome(code, &lines).map(|_| ())
}

pub fn uninstall(id: &str, scope: &str, cwd: &Path) -> Result<()> {
    let (code, lines) = claude(&["plugin".into(), "uninstall".into(), id.into(), "--scope".into(), scope.into(), "--json".into()], cwd)?;
    outcome(code, &lines).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{Confirmation, Outcome, outcome};

    #[test]
    fn lit_les_reponses() {
        let ok = outcome(Some(0), &["installation…".into(), r#"{"command":"install","outcome":"success","message":"Installé"}"#.into()]).unwrap();
        assert_eq!(ok, Outcome::Done("Installé".into()));
        let failed = outcome(Some(0), &[r#"{"outcome":"failed","message":"Plugin introuvable","failureCode":"not_found"}"#.into()]);
        assert_eq!(failed.unwrap_err().to_string(), "Plugin introuvable");
        let confirm = outcome(Some(1), &[r#"{"outcome":"failed","shownCommand":{"command":"npm i -g x","sha256":"abc"}}"#.into()]).unwrap();
        assert_eq!(confirm, Outcome::NeedsConfirmation(Confirmation { command: "npm i -g x".into(), sha256: "abc".into() }));
    }
}
