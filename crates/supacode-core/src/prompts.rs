//! Bibliothèque de prompts du projet : `.supacode/prompts/<id>.md`, versionnés avec le code.
//! Un prompt est un livrable : relu, commenté, historisé par git comme le reste.
//!
//! En-tête facultatif :
//! ```text
//! ---
//! titre: Corrige les tests
//! description: Fait passer les tests en échec sans les modifier.
//! agent: auto
//! variables: fichier, ticket
//! ---
//! Corps du prompt, avec des {{variables}}.
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use crate::generate::slug;
use crate::process::{self, Spawn};

#[derive(Debug, Clone, PartialEq)]
pub struct PromptFile {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    /// `auto`, `claude` ou `codex`.
    pub agent: Option<String>,
    /// Variables à demander au lancement (en-tête, puis `{{…}}` du corps).
    pub variables: Vec<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    pub commit: String,
    pub date: String,
    pub author: String,
    pub subject: String,
}

pub fn dir(root: &Path) -> PathBuf {
    root.join(".supacode").join("prompts")
}

pub fn relative_path(id: &str) -> String {
    format!(".supacode/prompts/{id}.md")
}

pub fn parse(id: &str, text: &str) -> PromptFile {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let (header, body) = match text.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---")) {
        Some((header, rest)) => (header.to_owned(), rest.strip_prefix('\n').unwrap_or(rest).trim_start_matches('\n').to_owned()),
        None => (String::new(), text.clone()),
    };
    let mut fields = HashMap::new();
    for line in header.lines() {
        if let Some((key, value)) = line.split_once(':') {
            fields.insert(key.trim().to_lowercase(), value.trim().to_owned());
        }
    }
    let field = |keys: &[&str]| keys.iter().find_map(|k| fields.get(*k)).filter(|v| !v.is_empty()).cloned();
    let mut variables: Vec<String> = field(&["variables"])
        .map(|v| v.trim_matches(['[', ']']).split(',').map(|s| s.trim().trim_matches(['"', '\'']).to_owned()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    for name in variables_in(&body) {
        if !variables.contains(&name) {
            variables.push(name);
        }
    }
    let title = field(&["titre", "title"]).unwrap_or_else(|| body.lines().find(|l| !l.trim().is_empty()).map(|l| l.trim().trim_start_matches('#').trim().chars().take(60).collect()).unwrap_or_else(|| id.to_owned()));
    PromptFile { id: id.to_owned(), title, description: field(&["description"]), agent: field(&["agent"]), variables, body: body.trim_end().to_owned() }
}

/// `{{nom}}` du texte, hors variables fournies par SupaCode (`steps.…`, `git.…`).
pub fn variables_in(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let Some(end) = rest[start + 2..].find("}}") else { break };
        let name = rest[start + 2..start + 2 + end].trim().to_owned();
        let reserved = name.starts_with("steps.") || name.starts_with("git.");
        if !name.is_empty() && !reserved && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.') && !names.contains(&name) {
            names.push(name);
        }
        rest = &rest[start + 2 + end + 2..];
    }
    names
}

pub fn load_all(root: &Path) -> Result<Vec<PromptFile>> {
    let Ok(entries) = std::fs::read_dir(dir(root)) else { return Ok(Vec::new()) };
    let mut prompts = Vec::new();
    for path in entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "md")) {
        let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let text = std::fs::read_to_string(&path).with_context(|| format!("lecture de {}", path.display()))?;
        prompts.push(parse(&id, &text));
    }
    prompts.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(prompts)
}

pub fn load(root: &Path, id: &str) -> Result<PromptFile> {
    if id.is_empty() || !id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        bail!("nom de prompt invalide : « {id} »");
    }
    let path = dir(root).join(format!("{id}.md"));
    let text = std::fs::read_to_string(&path).with_context(|| format!("prompt « {id} » introuvable ({})", relative_path(id)))?;
    Ok(parse(id, &text))
}

/// Enregistre un nouveau prompt ; ne remplace jamais un fichier existant. Renvoie son id.
pub fn save(root: &Path, title: &str, description: Option<&str>, body: &str) -> Result<String> {
    let dir = dir(root);
    std::fs::create_dir_all(&dir).with_context(|| format!("impossible de créer {}", dir.display()))?;
    let base = slug(title);
    let base = if base.is_empty() { "prompt".to_owned() } else { base };
    let id = (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|id| !dir.join(format!("{id}.md")).exists()).unwrap_or(base);
    let title = title.replace('\n', " ");
    let mut text = format!("---\ntitre: {title}\n");
    if let Some(description) = description {
        text.push_str(&format!("description: {}\n", description.replace('\n', " ")));
    }
    text.push_str("agent: auto\n---\n");
    text.push_str(body.trim());
    text.push('\n');
    std::fs::write(dir.join(format!("{id}.md")), text)?;
    Ok(id)
}

pub fn template() -> &'static str {
    "Décris ici ce que l'agent doit faire, comme tu l'expliquerais à un collègue.\n\nLes variables s'écrivent {{comme_ceci}} : SupaCode les demande au lancement.\n"
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let mut out = String::new();
    let spawn = Spawn::program("git", args.iter().map(|a| (*a).to_owned()).collect(), root);
    let code = process::run_lines(spawn, &AtomicBool::new(false), |line| {
        if !line.stderr {
            out.push_str(&line.text);
            out.push('\n');
        }
    })
    .ok()??;
    (code == 0).then_some(out)
}

/// Versions du prompt, de la plus récente à la plus ancienne (historique git).
pub fn history(root: &Path, id: &str) -> Vec<Version> {
    let path = relative_path(id);
    let Some(out) = git(root, &["log", "--follow", "--date=short", "--format=%h\x1f%ad\x1f%an\x1f%s", "--", &path]) else { return Vec::new() };
    out.lines()
        .filter_map(|line| {
            let mut parts = line.split('\x1f');
            Some(Version { commit: parts.next()?.to_owned(), date: parts.next()?.to_owned(), author: parts.next()?.to_owned(), subject: parts.next().unwrap_or("").to_owned() })
        })
        .collect()
}

/// Texte du prompt à une version donnée.
pub fn at_version(root: &Path, id: &str, commit: &str) -> Option<String> {
    let text = git(root, &["show", &format!("{commit}:{}", relative_path(id))])?;
    Some(parse(id, &text).body)
}

/// Version courante : dernier commit du fichier, suivi de « + modifs » s'il a changé depuis.
pub fn version(root: &Path, id: &str) -> String {
    let path = relative_path(id);
    let commit = git(root, &["log", "-1", "--format=%h", "--", &path]).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let dirty = git(root, &["status", "--porcelain", "--", &path]).is_some_and(|s| !s.trim().is_empty());
    match (commit, dirty) {
        (Some(commit), false) => commit,
        (Some(commit), true) => format!("{commit} + modifs"),
        (None, _) => "non versionné".into(),
    }
}

/// Dernière demande écrite par l'utilisateur dans un transcript de Claude Code
/// (hors commandes, rappels système et résultats d'outils).
pub fn last_user_message(transcript: &Path) -> Option<String> {
    let text = std::fs::read_to_string(transcript).ok()?;
    text.lines().rev().find_map(|line| {
        if !line.contains("\"user\"") {
            return None;
        }
        let entry: Value = serde_json::from_str(line).ok()?;
        if entry.get("type").and_then(Value::as_str) != Some("user") || entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        let content = entry.get("message")?.get("content")?;
        let text = match content {
            Value::String(s) => s.clone(),
            Value::Array(blocks) => blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("text")).filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
            _ => return None,
        };
        let text = text.trim();
        (!text.is_empty() && !text.starts_with('<')).then(|| text.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::{parse, variables_in};

    #[test]
    fn lit_l_en_tete() {
        let p = parse("corrige", "---\ntitre: Corrige les tests\ndescription: Sans les modifier\nagent: claude\nvariables: [fichier]\n---\nCorrige {{fichier}} pour {{ticket}} ({{steps.a.output}}).\n");
        assert_eq!(p.title, "Corrige les tests");
        assert_eq!(p.agent.as_deref(), Some("claude"));
        assert_eq!(p.variables, ["fichier", "ticket"]);
        assert_eq!(p.body, "Corrige {{fichier}} pour {{ticket}} ({{steps.a.output}}).");
        let bare = parse("x", "# Revue de sécurité\nRelis le diff.");
        assert_eq!(bare.title, "Revue de sécurité");
        assert_eq!(variables_in("{{a}} {{ b }} {{git.diff}} {{a}}"), ["a", "b"]);
    }
}
