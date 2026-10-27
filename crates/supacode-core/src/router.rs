//! Orchestrateur : classe une demande (type + taille) et choisit l'agent et le modèle.
//! Même logique et même `.supacode/config.json` que le cœur TypeScript.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentId {
    Claude,
    Codex,
}

impl AgentId {
    pub fn label(self) -> &'static str {
        match self {
            AgentId::Claude => "Claude",
            AgentId::Codex => "Codex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskKind {
    Question,
    Review,
    SmallFix,
    Refactor,
    Feature,
}

impl TaskKind {
    pub fn label(self) -> &'static str {
        match self {
            TaskKind::Question => "question",
            TaskKind::Review => "review",
            TaskKind::SmallFix => "small-fix",
            TaskKind::Refactor => "refactor",
            TaskKind::Feature => "feature",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskSize {
    Tiny,
    Medium,
    Large,
}

impl TaskSize {
    pub fn label(self) -> &'static str {
        match self {
            TaskSize::Tiny => "tiny",
            TaskSize::Medium => "medium",
            TaskSize::Large => "large",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub kind: TaskKind,
    pub size: TaskSize,
    /// Indices qui ont mené à la décision, pour l'explication affichée.
    pub signals: Vec<String>,
}

/// Mot-clé : une des formes, entourée de limites de mot (ou préfixe si `prefix`).
struct Keyword {
    label: &'static str,
    forms: &'static [&'static str],
    prefix: bool,
}

const fn kw(label: &'static str, forms: &'static [&'static str], prefix: bool) -> Keyword {
    Keyword { label, forms, prefix }
}

// Ordre = priorité : le premier type qui a un indice l'emporte.
const KIND_KEYWORDS: &[(TaskKind, &[Keyword])] = &[
    (TaskKind::Review, &[kw("review", &["review"], true), kw("relis", &["relis", "relire", "relisez", "relecture"], true), kw("audit", &["audit"], true)]),
    (
        TaskKind::Question,
        &[
            kw("pourquoi", &["pourquoi"], false),
            kw("comment", &["comment"], false),
            kw("explique", &["expliqu"], true),
            kw("why", &["why"], false),
            kw("how", &["how"], false),
            kw("what", &["what"], false),
            kw("explain", &["explain"], true),
        ],
    ),
    (
        TaskKind::SmallFix,
        &[
            kw("typo", &["typo", "typos"], false),
            kw("fix", &["fix", "fixe", "fixes", "fixed", "fixing"], false),
            kw("corrige", &["corrig"], true),
            kw("renomme", &["renomm"], true),
            kw("rename", &["renam"], true),
        ],
    ),
    (
        TaskKind::Refactor,
        &[
            kw("refacto", &["refacto", "refactor"], true),
            kw("nettoie", &["nettoi", "nettoy"], true),
            kw("simplifie", &["simplifi"], true),
            kw("clean up", &["clean up", "cleanup"], false),
            kw("simplify", &["simplify"], false),
        ],
    ),
    (
        TaskKind::Feature,
        &[
            kw("ajoute", &["ajout"], true),
            kw("crée", &["crée", "cree", "créé", "creé"], true),
            kw("implémente", &["implément", "implement"], true),
            kw("add", &["add"], false),
            kw("create", &["create"], false),
            kw("implement", &["implement"], false),
        ],
    ),
];

const LARGE_KEYWORDS: &[Keyword] = &[
    kw(
        "tout le projet",
        &[
            "tout le projet", "toute l'app", "toute l'application", "tout le code", "toute la base de code", "toute la codebase",
            "tout le repo", "tout le dépôt", "toute l’app", "toute l’application", "toute la base", "tout l'app",
        ],
        false,
    ),
    kw(
        "whole project",
        &["whole project", "entire project", "whole app", "entire app", "whole codebase", "entire codebase", "whole repo", "entire repo"],
        false,
    ),
    kw("architecture", &["architectur"], true),
    kw("migration", &["migr"], true),
];

const TINY_KEYWORDS: &[Keyword] = &[
    kw("juste", &["juste", "just"], false),
    kw("simple", &["simple"], false),
    kw("petit", &["petit", "petite", "petits", "petites"], false),
    kw("typo", &["typo", "typos"], false),
];

const LARGE_WORDS: usize = 120;
const TINY_WORDS: usize = 12;

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn matches(text: &str, keyword: &Keyword) -> bool {
    keyword.forms.iter().any(|form| {
        text.match_indices(form).any(|(start, found)| {
            let before = text[..start].chars().next_back().is_none_or(|c| !is_word_char(c));
            let after = keyword.prefix || text[start + found.len()..].chars().next().is_none_or(|c| !is_word_char(c));
            before && after
        })
    })
}

fn first_match<'k>(text: &str, keywords: &'k [Keyword]) -> Option<&'k Keyword> {
    keywords.iter().find(|k| matches(text, k))
}

/// Classe une demande (type + taille) à partir de mots-clés français et anglais.
pub fn classify(prompt: &str) -> Classification {
    let text = prompt.trim().to_lowercase();
    let mut signals = Vec::new();

    let mut kind = None;
    for (candidate, keywords) in KIND_KEYWORDS {
        if let Some(hit) = first_match(&text, keywords) {
            kind = Some(*candidate);
            signals.push(format!("mot-clé « {} » → {}", hit.label, candidate.label()));
            break;
        }
        if *candidate == TaskKind::Question && text.ends_with('?') {
            kind = Some(TaskKind::Question);
            signals.push("se termine par « ? » → question".into());
            break;
        }
    }
    let kind = kind.unwrap_or_else(|| {
        signals.push("aucun mot-clé → feature par défaut".into());
        TaskKind::Feature
    });

    let words = text.split_whitespace().count();
    let size = if let Some(large) = first_match(&text, LARGE_KEYWORDS) {
        signals.push(format!("mot-clé « {} » → large", large.label));
        TaskSize::Large
    } else if words > LARGE_WORDS {
        signals.push(format!("{words} mots (> {LARGE_WORDS}) → large"));
        TaskSize::Large
    } else if let Some(tiny) = first_match(&text, TINY_KEYWORDS) {
        signals.push(format!("mot-clé « {} » → tiny", tiny.label));
        TaskSize::Tiny
    } else if kind == TaskKind::SmallFix {
        signals.push("petite correction → tiny".into());
        TaskSize::Tiny
    } else if words < TINY_WORDS {
        signals.push(format!("{words} mots (< {TINY_WORDS}) → tiny"));
        TaskSize::Tiny
    } else {
        signals.push(format!("{words} mots → medium"));
        TaskSize::Medium
    };

    Classification { kind, size, signals }
}

/// Agent + modèle (alias de la CLI, ex. « sonnet » pour Claude Code).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentChoice {
    pub agent: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Règle de routage : s'applique si `kind` et `size` (facultatifs) correspondent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteRule {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TaskKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<TaskSize>,
    pub agent: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrchestratorConfig {
    /// Si faux, toutes les demandes partent vers `fixed`.
    pub enabled: bool,
    pub fixed: AgentChoice,
    /// Évaluées dans l'ordre : la première qui correspond gagne.
    pub rules: Vec<RouteRule>,
    /// Utilisé quand aucune règle ne correspond.
    pub fallback: AgentChoice,
}

fn rule(name: &str, kind: Option<TaskKind>, size: Option<TaskSize>, agent: AgentId, model: Option<&str>) -> RouteRule {
    RouteRule { name: name.into(), kind, size, agent, model: model.map(Into::into) }
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        use AgentId::*;
        Self {
            enabled: true,
            fixed: AgentChoice { agent: Claude, model: Some("sonnet".into()) },
            rules: vec![
                rule("small-fix + tiny", Some(TaskKind::SmallFix), Some(TaskSize::Tiny), Codex, None),
                rule("question", Some(TaskKind::Question), None, Claude, Some("haiku")),
                rule("review", Some(TaskKind::Review), None, Claude, Some("sonnet")),
                rule("large", None, Some(TaskSize::Large), Claude, Some("opus")),
                rule("refactor", Some(TaskKind::Refactor), None, Claude, Some("opus")),
                rule("tiny", None, Some(TaskSize::Tiny), Codex, None),
            ],
            fallback: AgentChoice { agent: Claude, model: Some("sonnet".into()) },
        }
    }
}

/// Fichier `.supacode/config.json` : toutes les clés sont facultatives.
#[derive(Deserialize, Default)]
struct ConfigFile {
    #[serde(default)]
    orchestrator: Option<PartialOrchestrator>,
}

#[derive(Deserialize)]
struct PartialOrchestrator {
    enabled: Option<bool>,
    fixed: Option<PartialChoice>,
    rules: Option<Vec<RouteRule>>,
    fallback: Option<PartialChoice>,
}

#[derive(Deserialize)]
struct PartialChoice {
    agent: Option<AgentId>,
    model: Option<String>,
}

fn merge_choice(base: AgentChoice, user: Option<PartialChoice>) -> AgentChoice {
    match user {
        Some(user) => AgentChoice { agent: user.agent.unwrap_or(base.agent), model: user.model.or(base.model) },
        None => base,
    }
}

/// Lit `.supacode/config.json` du projet ; configuration par défaut si le fichier n'existe pas.
pub fn load_config(project: &Path) -> Result<OrchestratorConfig> {
    let path = project.join(".supacode").join("config.json");
    let base = OrchestratorConfig::default();
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(base);
    };
    let file: ConfigFile = serde_json::from_str(&text).with_context(|| format!("configuration invalide dans {}", path.display()))?;
    let Some(user) = file.orchestrator else {
        return Ok(base);
    };
    Ok(OrchestratorConfig {
        enabled: user.enabled.unwrap_or(base.enabled),
        fixed: merge_choice(base.fixed, user.fixed),
        rules: user.rules.unwrap_or(base.rules),
        fallback: merge_choice(base.fallback, user.fallback),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteDecision {
    pub agent: AgentId,
    pub model: Option<String>,
    pub reason: String,
    pub classification: Classification,
}

/// Décide quel agent et quel modèle traitent la demande ; `agent`/`model` forcés passent en priorité.
pub fn route(prompt: &str, config: &OrchestratorConfig, agent: Option<AgentId>, model: Option<&str>) -> RouteDecision {
    let classification = classify(prompt);
    if let Some(agent) = agent {
        return RouteDecision { agent, model: model.map(Into::into), reason: "agent forcé manuellement".into(), classification };
    }
    let (kind, size) = (classification.kind, classification.size);
    let (choice, mut reason) = if !config.enabled {
        (config.fixed.clone(), "orchestrateur désactivé → agent fixe".to_owned())
    } else if let Some(rule) = config.rules.iter().find(|r| r.kind.is_none_or(|k| k == kind) && r.size.is_none_or(|s| s == size)) {
        (AgentChoice { agent: rule.agent, model: rule.model.clone() }, format!("{} / {} → règle {}", kind.label(), size.label(), rule.name))
    } else {
        (config.fallback.clone(), format!("{} / {} → repli", kind.label(), size.label()))
    };
    let mut model_choice = choice.model;
    if let Some(model) = model {
        model_choice = Some(model.into());
        reason.push_str(" (modèle forcé)");
    }
    RouteDecision { agent: choice.agent, model: model_choice, reason, classification }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classe_comme_le_coeur_typescript() {
        let c = classify("corrige la typo dans le README");
        assert_eq!((c.kind, c.size), (TaskKind::SmallFix, TaskSize::Tiny));
        let c = classify("Pourquoi le build échoue ?");
        assert_eq!(c.kind, TaskKind::Question);
        let c = classify("refactorise tout le projet pour séparer les couches métier et la persistance des données");
        assert_eq!((c.kind, c.size), (TaskKind::Refactor, TaskSize::Large));
        // « commentaire » ne doit pas déclencher « comment ».
        assert_eq!(classify("ajoute un commentaire").kind, TaskKind::Feature);
        let c = classify("relis les changements du panier et signale les régressions possibles côté paiement s'il y en a");
        assert_eq!((c.kind, c.size), (TaskKind::Review, TaskSize::Medium));
    }

    #[test]
    fn route_selon_les_regles() {
        let config = OrchestratorConfig::default();
        let d = route("corrige la typo", &config, None, None);
        assert_eq!((d.agent, d.model.as_deref()), (AgentId::Codex, None));
        let d = route("relis ce diff et dis-moi ce qui pourrait casser en production chez les clients", &config, None, None);
        assert_eq!((d.agent, d.model.as_deref()), (AgentId::Claude, Some("sonnet")));
        let d = route("corrige la typo", &config, Some(AgentId::Claude), Some("opus"));
        assert_eq!((d.agent, d.model.as_deref()), (AgentId::Claude, Some("opus")));
    }
}
