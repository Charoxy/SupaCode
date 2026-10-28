//! Extensions des agents : skills (dossiers `SKILL.md`) et serveurs MCP.
//!
//! SupaCode ne réécrit pas la configuration de Claude Code : ce qu'on désactive ici est
//! appliqué aux sessions qu'il lance, par `--settings` (`skillOverrides`, interdiction des
//! outils `mcp__<serveur>`, `disabledMcpjsonServers`). La bibliothèque cherche sur SkillsMP
//! et dans le dépôt officiel d'Anthropic ; l'installation télécharge le dossier du skill
//! depuis GitHub.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::process::{self, Spawn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `~/.claude/skills` : pour soi, dans tous les projets.
    User,
    /// `<projet>/.claude/skills` : versionné, partagé avec l'équipe.
    Project,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::User => "moi",
            Scope::Project => "projet",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub scope: Scope,
    pub dir: PathBuf,
    pub enabled: bool,
    /// Adresse d'origine, pour un skill installé par SupaCode.
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpServer {
    pub name: String,
    /// « utilisateur », « local », « projet (.mcp.json) » ou « SupaCode ».
    pub scope: &'static str,
    /// Commande ou adresse.
    pub detail: String,
    pub enabled: bool,
}

/// Réglages de SupaCode pour les extensions (`%APPDATA%\SupaCode\extensions.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub skills_off: Vec<String>,
    #[serde(default)]
    pub mcp_off: Vec<String>,
    /// Skills installés par SupaCode : dossier → adresse d'origine.
    #[serde(default)]
    pub installed: BTreeMap<String, String>,
    /// Clé de l'API SkillsMP (facultative : sans clé, 50 recherches par jour).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skillsmp_key: Option<String>,
}

fn settings_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("SupaCode").join("extensions.json"))
}

pub fn load_settings() -> Settings {
    settings_file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> Result<()> {
    let file = settings_file().ok_or_else(|| anyhow!("dossier de configuration introuvable"))?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, serde_json::to_string_pretty(settings)?)?;
    Ok(())
}

impl Settings {
    /// Réglages à passer à `claude --settings` pour appliquer les désactivations.
    pub fn claude_settings(&self, project_mcp: &[String]) -> Option<Value> {
        let mut settings = serde_json::Map::new();
        if !self.skills_off.is_empty() {
            let overrides: serde_json::Map<String, Value> = self.skills_off.iter().map(|s| (s.clone(), json!("off"))).collect();
            settings.insert("skillOverrides".into(), Value::Object(overrides));
        }
        // Les noms d'outils remplacent « . » et les espaces par « _ ».
        let deny: Vec<Value> = self.mcp_off.iter().filter(|s| *s != SUPACODE_SERVER).map(|s| json!(format!("mcp__{}", s.replace(['.', ' '], "_")))).collect();
        if !deny.is_empty() {
            settings.insert("permissions".into(), json!({ "deny": deny }));
        }
        let rejected: Vec<&String> = self.mcp_off.iter().filter(|s| project_mcp.contains(s)).collect();
        if !rejected.is_empty() {
            settings.insert("disabledMcpjsonServers".into(), json!(rejected));
        }
        (!settings.is_empty()).then_some(Value::Object(settings))
    }
}

/// Fusionne deux réglages `--settings` (les listes d'interdictions s'additionnent).
pub fn merge_settings(a: Option<Value>, b: Option<Value>) -> Option<Value> {
    match (a, b) {
        (Some(Value::Object(mut a)), Some(Value::Object(b))) => {
            for (key, value) in b {
                match (a.get_mut(&key), value) {
                    (Some(Value::Object(existing)), Value::Object(more)) => {
                        for (k, v) in more {
                            match (existing.get_mut(&k), v) {
                                (Some(Value::Array(list)), Value::Array(extra)) => list.extend(extra),
                                (_, v) => {
                                    existing.insert(k, v);
                                }
                            }
                        }
                    }
                    (_, value) => {
                        a.insert(key, value);
                    }
                }
            }
            Some(Value::Object(a))
        }
        (Some(a), None) => Some(a),
        (None, b) => b,
        (Some(a), Some(_)) => Some(a),
    }
}

/// Nom du serveur MCP du navigateur intégré.
pub const SUPACODE_SERVER: &str = "supacode";

fn frontmatter(text: &str) -> BTreeMap<String, String> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let Some(header) = text.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---")).map(|(h, _)| h.to_owned()) else { return BTreeMap::new() };
    let mut fields = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in header.lines() {
        // Valeur sur plusieurs lignes (`description: >`) : les lignes indentées suivent.
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(key) = &current {
                let entry: &mut String = fields.entry(key.clone()).or_default();
                if !entry.is_empty() {
                    entry.push(' ');
                }
                entry.push_str(line.trim());
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim().trim_matches(['"', '\'']);
            let value = if matches!(value, ">" | "|" | ">-" | "|-") { "" } else { value };
            fields.insert(key.trim().to_owned(), value.to_owned());
            current = Some(key.trim().to_owned());
        }
    }
    fields
}

fn user_skills_dir() -> Option<PathBuf> {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".claude")))?;
    Some(base.join("skills"))
}

pub fn skills_dir(root: Option<&Path>, scope: Scope) -> Option<PathBuf> {
    match scope {
        Scope::User => user_skills_dir(),
        Scope::Project => root.map(|r| r.join(".claude").join("skills")),
    }
}

/// Skills de l'utilisateur et du projet.
pub fn list_skills(root: Option<&Path>, settings: &Settings) -> Vec<Skill> {
    let mut skills = Vec::new();
    for scope in [Scope::Project, Scope::User] {
        let Some(dir) = skills_dir(root, scope) else { continue };
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(text) = std::fs::read_to_string(path.join("SKILL.md")) else { continue };
            let fields = frontmatter(&text);
            let folder = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let name = fields.get("name").filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| folder.clone());
            skills.push(Skill {
                enabled: !settings.skills_off.contains(&name),
                source: settings.installed.get(&path.display().to_string()).cloned(),
                description: fields.get("description").cloned().unwrap_or_default(),
                name,
                scope,
                dir: path,
            });
        }
    }
    skills.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    skills
}

fn server_detail(config: &Value) -> String {
    if let Some(url) = config.get("url").and_then(Value::as_str) {
        return url.to_owned();
    }
    let command = config.get("command").and_then(Value::as_str).unwrap_or("");
    let args: Vec<&str> = config.get("args").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    format!("{command} {}", args.join(" ")).trim().to_owned()
}

/// Noms des serveurs du `.mcp.json` du projet.
pub fn project_servers(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join(".mcp.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("mcpServers").and_then(Value::as_object).map(|m| m.keys().cloned().collect()))
        .unwrap_or_default()
}

/// Serveurs MCP vus par Claude Code dans ce projet, plus celui du navigateur de SupaCode.
pub fn list_mcp(root: Option<&Path>, settings: &Settings) -> Vec<McpServer> {
    let mut servers = vec![McpServer { name: SUPACODE_SERVER.into(), scope: "SupaCode", detail: "navigateur intégré".into(), enabled: !settings.mcp_off.iter().any(|s| s == SUPACODE_SERVER) }];
    let mut push = |name: &str, scope: &'static str, config: &Value| {
        if !servers.iter().any(|s| s.name == name) {
            servers.push(McpServer { name: name.to_owned(), scope, detail: server_detail(config), enabled: !settings.mcp_off.iter().any(|s| s == name) });
        }
    };
    let global = dirs::home_dir().and_then(|h| std::fs::read_to_string(h.join(".claude.json")).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok());
    if let Some(global) = &global {
        if let (Some(root), Some(projects)) = (root, global.get("projects").and_then(Value::as_object)) {
            let key = root.display().to_string().replace('\\', "/").to_lowercase();
            let local = projects.iter().find(|(path, _)| path.replace('\\', "/").to_lowercase() == key).and_then(|(_, p)| p.get("mcpServers")).and_then(Value::as_object);
            for (name, config) in local.into_iter().flatten() {
                push(name, "local", config);
            }
        }
        for (name, config) in global.get("mcpServers").and_then(Value::as_object).into_iter().flatten() {
            push(name, "utilisateur", config);
        }
    }
    if let Some(root) = root {
        let project = std::fs::read_to_string(root.join(".mcp.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        for (name, config) in project.as_ref().and_then(|v| v.get("mcpServers")).and_then(Value::as_object).into_iter().flatten() {
            push(name, "projet (.mcp.json)", config);
        }
    }
    servers
}

// — Bibliothèque

#[derive(Debug, Clone, PartialEq)]
pub struct MarketSkill {
    pub name: String,
    pub author: String,
    pub description: String,
    /// Dossier du skill sur GitHub (`…/tree/<branche>/<chemin>`).
    pub github_url: String,
    pub page: Option<String>,
    pub stars: Option<u64>,
}

/// Requête HTTP GET (via `curl`, présent sous Windows 10+, macOS et Linux).
fn get(url: &str, headers: &[String]) -> Result<(u16, String)> {
    let mut args = vec!["-sL".to_owned(), "-m".into(), "30".into(), "-A".into(), "SupaCode".into(), "-w".into(), "\n%{http_code}".into()];
    for header in headers {
        args.extend(["-H".into(), header.clone()]);
    }
    args.push(url.to_owned());
    let mut out = Vec::new();
    let cwd = std::env::temp_dir();
    let spawn = Spawn::program("curl", args, &cwd);
    let code = process::run_lines(spawn, &AtomicBool::new(false), |line| {
        if !line.stderr {
            out.push(line.text);
        }
    })?;
    if code != Some(0) {
        bail!("pas de réponse de {url} (réseau ?)");
    }
    let status = out.pop().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    Ok((status, out.join("\n")))
}

/// Recherche sur SkillsMP (`GET /api/v1/skills/search`).
pub fn search_skillsmp(query: &str, key: Option<&str>) -> Result<Vec<MarketSkill>> {
    let q: String = query.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let headers: Vec<String> = key.filter(|k| !k.is_empty()).map(|k| format!("Authorization: Bearer {k}")).into_iter().collect();
    let (status, body) = get(&format!("https://skillsmp.com/api/v1/skills/search?q={q}&limit=30&sortBy=stars"), &headers)?;
    let value: Value = serde_json::from_str(&body).with_context(|| format!("réponse inattendue de SkillsMP (HTTP {status})"))?;
    if value.get("success").and_then(Value::as_bool) != Some(true) {
        let error = value.get("error");
        let code = error.and_then(|e| e.get("code")).and_then(Value::as_str).unwrap_or("");
        let message = match code {
            "DAILY_QUOTA_EXCEEDED" | "RATE_LIMITED" | "MONTHLY_QUOTA_EXCEEDED" => "Quota de recherches SkillsMP atteint (50 par jour sans clé). Ajoute une clé API SkillsMP, ou réessaie plus tard.".to_owned(),
            "INVALID_API_KEY" => "Clé API SkillsMP invalide.".to_owned(),
            _ => error.and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("erreur SkillsMP").to_owned(),
        };
        bail!("{message}");
    }
    let skills = value.pointer("/data/skills").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(skills
        .iter()
        .filter_map(|s| {
            let text = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
            let github_url = text("githubUrl");
            (!github_url.is_empty()).then(|| MarketSkill {
                name: text("name"),
                author: text("author"),
                description: text("description"),
                github_url,
                page: s.get("skillUrl").and_then(Value::as_str).map(str::to_owned),
                stars: s.get("stars").and_then(Value::as_u64),
            })
        })
        .collect())
}

/// Skills officiels d'Anthropic (`github.com/anthropics/skills`), gardés un jour en cache.
pub fn anthropic_catalog() -> Result<Vec<MarketSkill>> {
    let cache = dirs::cache_dir().map(|d| d.join("SupaCode").join("skills-anthropic.json"));
    let fresh = cache.as_ref().and_then(|c| std::fs::metadata(c).ok()).and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < 86_400);
    if fresh {
        if let Some(list) = cache.as_ref().and_then(|c| std::fs::read_to_string(c).ok()).and_then(|t| serde_json::from_str::<Vec<(String, String, String)>>(&t).ok()) {
            return Ok(list.into_iter().map(|(name, description, url)| MarketSkill { name, author: "anthropics".into(), description, github_url: url, page: None, stars: None }).collect());
        }
    }
    let (status, body) = get("https://api.github.com/repos/anthropics/skills/contents/skills", &["Accept: application/vnd.github+json".into()])?;
    if status != 200 {
        bail!("GitHub ne répond pas (HTTP {status}) : réessaie plus tard");
    }
    let entries: Vec<Value> = serde_json::from_str(&body)?;
    let mut list = Vec::new();
    for entry in entries.iter().filter(|e| e.get("type").and_then(Value::as_str) == Some("dir")) {
        let name = entry.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
        let raw = format!("https://raw.githubusercontent.com/anthropics/skills/main/skills/{name}/SKILL.md");
        let description = get(&raw, &[]).ok().filter(|(s, _)| *s == 200).map(|(_, text)| frontmatter(&text).get("description").cloned().unwrap_or_default()).unwrap_or_default();
        list.push((name.clone(), description, format!("https://github.com/anthropics/skills/tree/main/skills/{name}")));
    }
    if let Some(cache) = &cache {
        let _ = cache.parent().map(std::fs::create_dir_all);
        let _ = std::fs::write(cache, serde_json::to_string(&list).unwrap_or_default());
    }
    Ok(list.into_iter().map(|(name, description, url)| MarketSkill { name, author: "anthropics".into(), description, github_url: url, page: None, stars: None }).collect())
}

/// `https://github.com/<owner>/<repo>/tree/<ref>/<path>` → (owner, repo, ref, path).
pub fn parse_github_url(url: &str) -> Option<(String, String, String, String)> {
    let rest = url.trim().trim_end_matches('/').strip_prefix("https://github.com/")?;
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        [owner, repo, "tree" | "blob", reference, path @ ..] => {
            let path = path.join("/");
            // Lien vers le SKILL.md lui-même : on prend son dossier.
            let path = path.strip_suffix("/SKILL.md").or_else(|| path.strip_suffix("SKILL.md")).unwrap_or(&path).trim_end_matches('/').to_owned();
            Some(((*owner).into(), (*repo).into(), (*reference).into(), path))
        }
        [owner, repo] => Some(((*owner).into(), (*repo).into(), "HEAD".into(), String::new())),
        _ => None,
    }
}

/// Télécharge le dossier d'un skill depuis GitHub dans `dest/<nom>` ; renvoie le dossier créé.
/// Refuse un dossier sans `SKILL.md` et ne remplace jamais un skill existant.
pub fn install(github_url: &str, dest: &Path) -> Result<PathBuf> {
    let (owner, repo, reference, path) = parse_github_url(github_url).ok_or_else(|| anyhow!("adresse GitHub non reconnue : {github_url}"))?;
    let name = path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(&repo).to_owned();
    let target = dest.join(&name);
    if target.exists() {
        bail!("un skill « {name} » existe déjà ({})", target.display());
    }
    let mut files = Vec::new();
    collect(&owner, &repo, &reference, &path, "", &mut files, 0)?;
    if !files.iter().any(|(relative, _)| relative == "SKILL.md") {
        bail!("ce dossier ne contient pas de SKILL.md : ce n'est pas un skill");
    }
    let total: usize = files.len();
    if total > 200 {
        bail!("ce skill contient {total} fichiers : trop pour une installation automatique");
    }
    for (relative, download) in &files {
        let (status, body) = get_bytes(download)?;
        if status != 200 {
            let _ = std::fs::remove_dir_all(&target);
            bail!("téléchargement impossible de {relative} (HTTP {status})");
        }
        let file = target.join(relative);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file, body)?;
    }
    Ok(target)
}

fn collect(owner: &str, repo: &str, reference: &str, path: &str, prefix: &str, files: &mut Vec<(String, String)>, depth: usize) -> Result<()> {
    if depth > 4 {
        return Ok(());
    }
    let url = format!("https://api.github.com/repos/{owner}/{repo}/contents/{path}?ref={reference}");
    let (status, body) = get(&url, &["Accept: application/vnd.github+json".into()])?;
    match status {
        200 => {}
        403 | 429 => bail!("GitHub limite les téléchargements sans compte (60 par heure) : réessaie un peu plus tard"),
        404 => bail!("dossier introuvable sur GitHub : {owner}/{repo}/{path}"),
        _ => bail!("GitHub a répondu HTTP {status}"),
    }
    let entries: Vec<Value> = serde_json::from_str(&body).context("réponse inattendue de GitHub")?;
    for entry in entries {
        let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
        let relative = if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") };
        match entry.get("type").and_then(Value::as_str) {
            Some("file") => {
                if let Some(download) = entry.get("download_url").and_then(Value::as_str) {
                    files.push((relative, download.to_owned()));
                }
            }
            Some("dir") => {
                let sub = entry.get("path").and_then(Value::as_str).unwrap_or("");
                collect(owner, repo, reference, sub, &relative, files, depth + 1)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Téléchargement binaire (fichiers du skill) dans un fichier temporaire.
fn get_bytes(url: &str) -> Result<(u16, Vec<u8>)> {
    let temp = std::env::temp_dir().join(format!("supacode-dl-{}", std::process::id()));
    let args = vec!["-sL".to_owned(), "-m".into(), "60".into(), "-A".into(), "SupaCode".into(), "-o".into(), temp.display().to_string(), "-w".into(), "%{http_code}".into(), url.to_owned()];
    let mut status = String::new();
    let cwd = std::env::temp_dir();
    let spawn = Spawn::program("curl", args, &cwd);
    let code = process::run_lines(spawn, &AtomicBool::new(false), |line| {
        if !line.stderr {
            status.push_str(line.text.trim());
        }
    })?;
    if code != Some(0) {
        bail!("pas de réponse de {url}");
    }
    let body = std::fs::read(&temp).unwrap_or_default();
    let _ = std::fs::remove_file(&temp);
    Ok((status.parse().unwrap_or(0), body))
}

#[cfg(test)]
mod tests {
    use super::{Settings, frontmatter, merge_settings, parse_github_url};
    use serde_json::json;

    /// Réseau : `cargo test -p supacode-core -- --ignored`.
    #[test]
    #[ignore]
    fn recherche_skillsmp() {
        let found = super::search_skillsmp("pdf", None).unwrap();
        assert!(!found.is_empty());
        assert!(found.iter().all(|s| parse_github_url(&s.github_url).is_some()));
    }

    #[test]
    fn adresses_github() {
        assert_eq!(
            parse_github_url("https://github.com/openclaw/openclaw/tree/main/skills/nano-pdf"),
            Some(("openclaw".into(), "openclaw".into(), "main".into(), "skills/nano-pdf".into()))
        );
        assert_eq!(parse_github_url("https://github.com/a/b/blob/v2/x/SKILL.md").map(|p| p.3), Some("x".into()));
        assert_eq!(parse_github_url("https://exemple.com/a"), None);
    }

    #[test]
    fn en_tete_sur_plusieurs_lignes() {
        let f = frontmatter("---\nname: pdf\ndescription: >\n  Lit et remplit\n  des PDF.\nlicense: MIT\n---\n# PDF\n");
        assert_eq!(f.get("description").map(String::as_str), Some("Lit et remplit des PDF."));
        assert_eq!(f.get("name").map(String::as_str), Some("pdf"));
    }

    #[test]
    fn reglages_pour_claude() {
        let settings = Settings { skills_off: vec!["pdf".into()], mcp_off: vec!["stripe".into(), "supacode".into(), "my.db".into()], ..Settings::default() };
        let value = settings.claude_settings(&["my.db".into()]).unwrap();
        assert_eq!(value["skillOverrides"]["pdf"], "off");
        assert_eq!(value["permissions"]["deny"], json!(["mcp__stripe", "mcp__my_db"]));
        assert_eq!(value["disabledMcpjsonServers"], json!(["my.db"]));
        let merged = merge_settings(Some(json!({ "permissions": { "deny": ["Bash(rm -rf:*)"] } })), Some(value)).unwrap();
        assert_eq!(merged["permissions"]["deny"], json!(["Bash(rm -rf:*)", "mcp__stripe", "mcp__my_db"]));
    }
}
