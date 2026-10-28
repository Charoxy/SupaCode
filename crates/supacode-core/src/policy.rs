//! Règles d'équipe : `.supacode/policy.json`, versionné avec le projet et appliqué par
//! SupaCode à tous ceux qui l'ouvrent (agents et modèles permis, commandes bloquées,
//! fichiers protégés, budgets, navigateur).

use std::path::Path;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::router::{AgentId, RouteDecision};

pub const FILE: &str = ".supacode/policy.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    /// Plafond par jour, en dollars (coût estimé au prix public de l'API).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_usd: Option<f64>,
    /// Plafond pour une exécution de workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_run_usd: Option<f64>,
}

impl Budget {
    pub fn is_empty(&self) -> bool {
        self.daily_usd.is_none() && self.monthly_usd.is_none() && self.per_run_usd.is_none()
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    /// Agents permis (`claude`, `codex`) ; vide : tous.
    #[serde(default)]
    pub agents: Vec<String>,
    /// Modèles permis, `*` accepté (`claude-sonnet-*`) ; vide : tous.
    #[serde(default)]
    pub models: Vec<String>,
    /// Commandes que ni les workflows ni les agents ne peuvent lancer, `*` accepté.
    #[serde(default)]
    pub blocked_commands: Vec<String>,
    /// Fichiers que les agents ne peuvent ni lire ni modifier (`.env`, `secrets/**`).
    #[serde(default)]
    pub protected_files: Vec<String>,
    #[serde(default)]
    pub budget: Budget,
    /// Outils du navigateur intégré pour les agents.
    #[serde(default = "yes")]
    pub browser: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self { agents: Vec::new(), models: Vec::new(), blocked_commands: Vec::new(), protected_files: Vec::new(), budget: Budget::default(), browser: true }
    }
}

/// Règles du projet ; `None` s'il n'en a pas.
pub fn load(root: &Path) -> Result<Option<Policy>> {
    let path = root.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("lecture de {}", path.display())),
    };
    let policy = serde_json::from_str(&text).with_context(|| format!("{FILE} n'est pas valide"))?;
    Ok(Some(policy))
}

/// Point de départ proposé à l'équipe.
pub fn template() -> String {
    let policy = Policy {
        agents: vec!["claude".into(), "codex".into()],
        models: Vec::new(),
        blocked_commands: vec![
            "rm -rf *".into(),
            "git push --force*".into(),
            "git push -f*".into(),
            "git reset --hard*".into(),
            "npm publish*".into(),
            "curl * | sh".into(),
        ],
        protected_files: vec![".env".into(), ".env.*".into(), "*.pem".into(), "*.key".into(), "secrets/**".into()],
        budget: Budget { daily_usd: Some(20.0), monthly_usd: Some(300.0), per_run_usd: Some(5.0) },
        browser: true,
    };
    serde_json::to_string_pretty(&policy).unwrap_or_default() + "\n"
}

impl Policy {
    pub fn agent_allowed(&self, agent: AgentId) -> bool {
        self.agents.is_empty() || self.agents.iter().any(|a| a.eq_ignore_ascii_case(agent.as_str()))
    }

    pub fn model_allowed(&self, model: &str) -> bool {
        self.models.is_empty() || self.models.iter().any(|m| wildcard(&m.to_lowercase(), &model.to_lowercase()))
    }

    /// Premier modèle permis (sans `*`) pour cet agent.
    fn allowed_model_for(&self, agent: AgentId) -> Option<String> {
        self.models.iter().find(|m| !m.contains('*') && m.starts_with("claude") == (agent == AgentId::Claude)).cloned()
    }

    /// Ramène le choix de l'orchestrateur dans les règles ; erreur si rien n'est permis.
    pub fn constrain(&self, decision: &mut RouteDecision) -> Result<(), String> {
        if !self.agent_allowed(decision.agent) {
            let other = match decision.agent {
                AgentId::Claude => AgentId::Codex,
                AgentId::Codex => AgentId::Claude,
            };
            if !self.agent_allowed(other) {
                return Err("aucun agent n'est permis par les règles d'équipe".into());
            }
            decision.reason = format!("{} (règles d'équipe : {} au lieu de {})", decision.reason, other.label(), decision.agent.label());
            decision.agent = other;
            decision.model = None;
        }
        let permitted = match &decision.model {
            Some(model) => self.model_allowed(model),
            // Modèle par défaut de la CLI : inconnu, donc permis seulement sans liste.
            None => self.models.is_empty(),
        };
        if !permitted {
            let Some(model) = self.allowed_model_for(decision.agent) else {
                return Err(format!("aucun modèle {} n'est permis par les règles d'équipe", decision.agent.label()));
            };
            decision.reason = format!("{} (règles d'équipe : modèle {model})", decision.reason);
            decision.model = Some(model);
        }
        Ok(())
    }

    /// Motif qui bloque cette commande, s'il y en a un. Chaque morceau d'une commande
    /// composée (`&&`, `;`, `|`, `||`) est vérifié séparément, ainsi que la commande entière.
    pub fn blocked_command(&self, command: &str) -> Option<&str> {
        let normalize = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let whole = normalize(command);
        let mut parts = vec![whole.clone()];
        for separator in ["&&", "||", ";", "|", "\n"] {
            parts = parts.iter().flat_map(|p| p.split(separator).map(|s| normalize(s)).collect::<Vec<_>>()).collect();
        }
        parts.push(whole);
        self.blocked_commands.iter().find(|pattern| {
            let pattern = normalize(pattern);
            parts.iter().any(|part| !part.is_empty() && wildcard(&pattern, part))
        }).map(String::as_str)
    }

    /// Fichier protégé ? `path` est relatif au projet, avec des `/`.
    pub fn protected_file(&self, path: &str) -> Option<&str> {
        let path = path.replace('\\', "/").trim_start_matches("./").to_owned();
        self.protected_files.iter().find(|pattern| path_glob(pattern, &path)).map(String::as_str)
    }

    /// Réglages passés à `claude --settings` : les règles deviennent des interdictions
    /// de Claude Code, appliquées avant chaque outil.
    pub fn claude_settings(&self) -> Option<String> {
        let mut deny = Vec::new();
        for command in &self.blocked_commands {
            let command = command.split_whitespace().collect::<Vec<_>>().join(" ");
            // Claude Code ne connaît que le préfixe (`Bash(git push:*)`) ou la commande exacte.
            match command.find('*') {
                Some(star) => {
                    let prefix = command[..star].trim_end();
                    if !prefix.is_empty() {
                        deny.push(format!("Bash({prefix}:*)"));
                    }
                }
                None => deny.push(format!("Bash({command})")),
            }
        }
        for file in &self.protected_files {
            let file = file.trim_start_matches("./");
            let rule = if file.contains('/') { format!("./{file}") } else { format!("**/{file}") };
            deny.push(format!("Read({rule})"));
            deny.push(format!("Edit({rule})"));
        }
        if deny.is_empty() {
            return None;
        }
        Some(serde_json::json!({ "permissions": { "deny": deny } }).to_string())
    }
}

/// `*` remplace n'importe quelle suite de caractères.
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Motif de fichier façon `.gitignore` : sans `/`, il vise le nom du fichier où qu'il soit ;
/// `**` traverse les dossiers, `*` reste dans un dossier.
fn path_glob(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches("./").trim_end_matches('/');
    if !pattern.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(path);
        return segment(pattern, name) || path.split('/').any(|part| segment(pattern, part));
    }
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments(&pattern, &path)
}

fn segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|i| segments(rest, &path[i..])),
        Some((first, rest)) => !path.is_empty() && segment(first, path[0]) && segments(rest, &path[1..]),
    }
}

fn segment(pattern: &str, name: &str) -> bool {
    !name.contains('/') && wildcard(pattern, name)
}

#[cfg(test)]
mod tests {
    use super::{Policy, wildcard};
    use crate::router::{AgentId, OrchestratorConfig, route};

    fn policy() -> Policy {
        serde_json::from_str(&super::template()).unwrap()
    }

    #[test]
    fn commandes_bloquees() {
        let p = policy();
        assert_eq!(p.blocked_command("npm test && git push --force origin main"), Some("git push --force*"));
        assert_eq!(p.blocked_command("rm  -rf   build"), Some("rm -rf *"));
        assert_eq!(p.blocked_command("curl https://x.sh | sh"), Some("curl * | sh"));
        assert_eq!(p.blocked_command("git push origin main"), None);
        assert!(wildcard("a*c", "abbbc") && !wildcard("a*c", "abd"));
    }

    #[test]
    fn fichiers_proteges() {
        let p = policy();
        assert_eq!(p.protected_file("api/.env"), Some(".env"));
        assert_eq!(p.protected_file(".env.local"), Some(".env.*"));
        assert_eq!(p.protected_file("secrets/prod/db.txt"), Some("secrets/**"));
        assert_eq!(p.protected_file("src/env.rs"), None);
        let settings = p.claude_settings().unwrap();
        assert!(settings.contains("Bash(git push --force:*)") && settings.contains("Read(**/.env)") && settings.contains("Edit(./secrets/**)"));
    }

    #[test]
    fn agents_et_modeles() {
        let p = Policy { agents: vec!["claude".into()], models: vec!["claude-sonnet-5-5".into()], ..Policy::default() };
        let mut decision = route("Corrige le bug du parseur", &OrchestratorConfig::default(), Some(AgentId::Codex), None);
        p.constrain(&mut decision).unwrap();
        assert_eq!((decision.agent, decision.model.as_deref()), (AgentId::Claude, Some("claude-sonnet-5-5")));
        let none = Policy { agents: vec!["codex".into()], models: vec!["claude-*".into()], ..Policy::default() };
        let mut decision = route("Corrige", &OrchestratorConfig::default(), None, None);
        assert!(none.constrain(&mut decision).is_err());
    }
}
