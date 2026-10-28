//! Coûts estimés des agents, au prix public de l'API Claude (avec un abonnement,
//! c'est une équivalence) : grille de prix, et lecture des transcripts de Claude Code
//! (`~/.claude/projects/<projet>/<session>.jsonl`) pour les dépenses et l'activité.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::Value;

/// Prix en dollars par million de jetons : entrée, sortie, lecture du cache.
/// L'écriture du cache coûte 1,25 × l'entrée (5 min) ou 2 × l'entrée (1 h).
/// Du plus précis au plus général : le premier préfixe qui correspond l'emporte.
const PRICES: &[(&str, f64, f64, f64)] = &[
    ("claude-fable-5-1", 10.0, 50.0, 0.25),
    ("claude-mythos-5-1", 10.0, 50.0, 0.25),
    ("claude-fable-5", 10.0, 50.0, 1.0),
    ("claude-mythos-5", 10.0, 50.0, 1.0),
    ("claude-opus-5-5", 4.0, 20.0, 0.20),
    ("claude-opus-5", 5.0, 25.0, 0.50),
    ("claude-opus-4-8", 5.0, 25.0, 0.50),
    ("claude-opus-4-7", 5.0, 25.0, 0.50),
    ("claude-opus-4-6", 5.0, 25.0, 0.50),
    ("claude-opus-4-5", 5.0, 25.0, 0.50),
    ("claude-opus-4", 15.0, 75.0, 1.50),
    ("claude-sonnet-5-5", 2.0, 10.0, 0.20),
    ("claude-sonnet-5", 2.0, 10.0, 0.20),
    ("claude-sonnet-4", 3.0, 15.0, 0.30),
    ("claude-haiku-5-5", 0.10, 0.50, 0.01),
    ("claude-haiku-4-5", 1.0, 5.0, 0.10),
    ("claude-3-5-haiku", 0.80, 4.0, 0.08),
];

/// Coût estimé d'une réponse, à partir de son champ `usage` ; `None` pour un modèle inconnu.
pub fn cost(model: &str, usage: &Value) -> Option<f64> {
    let model = model.to_lowercase();
    let &(_, input, output, cache_read) = PRICES.iter().find(|(prefix, ..)| model.starts_with(prefix))?;
    let tokens = |key: &str| usage.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    let creation = usage.get("cache_creation");
    let (hour, five) = match creation {
        Some(c) => (
            c.get("ephemeral_1h_input_tokens").and_then(Value::as_f64).unwrap_or(0.0),
            c.get("ephemeral_5m_input_tokens").and_then(Value::as_f64).unwrap_or(0.0),
        ),
        None => (0.0, tokens("cache_creation_input_tokens")),
    };
    let dollars = (tokens("input_tokens") * input
        + tokens("output_tokens") * output
        + tokens("cache_read_input_tokens") * cache_read
        + five * input * 1.25
        + hour * input * 2.0)
        / 1_000_000.0;
    // Mode rapide : tarif doublé.
    let fast = usage.get("speed").and_then(Value::as_str) == Some("fast");
    Some(if fast { dollars * 2.0 } else { dollars })
}

/// Une réponse d'agent facturée.
#[derive(Debug, Clone, PartialEq)]
pub struct Spend {
    /// Horodatage ISO 8601 (UTC).
    pub time: String,
    pub model: String,
    pub cost: f64,
    pub session: String,
}

/// Outil utilisé par l'agent dans une conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub time: String,
    pub session: String,
    pub tool: String,
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct Transcript {
    pub spends: Vec<Spend>,
    pub activity: Vec<Activity>,
}

/// Lit un transcript : une même réponse s'étale sur plusieurs lignes (une par bloc),
/// avec le même `usage` ; on ne la compte qu'une fois.
pub fn read_transcript(path: &Path) -> Transcript {
    let mut transcript = Transcript::default();
    let Ok(file) = std::fs::File::open(path) else { return transcript };
    let mut seen = HashSet::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("\"assistant\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };
        if entry.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let text = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_owned();
        let time = text(entry.get("timestamp"));
        let session = text(entry.get("sessionId"));
        let Some(message) = entry.get("message") else { continue };
        let model = text(message.get("model"));
        let id = text(message.get("id"));
        if let Some(usage) = message.get("usage") {
            if seen.insert(id) {
                if let Some(cost) = cost(&model, usage) {
                    transcript.spends.push(Spend { time: time.clone(), model: model.clone(), cost, session: session.clone() });
                }
            }
        }
        for block in message.get("content").and_then(Value::as_array).into_iter().flatten() {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let tool = text(block.get("name"));
            let input = block.get("input");
            let detail = ["command", "file_path", "notebook_path", "path", "url", "pattern"]
                .iter()
                .find_map(|key| input.and_then(|i| i.get(*key)).and_then(Value::as_str))
                .unwrap_or("")
                .to_owned();
            transcript.activity.push(Activity { time: time.clone(), session: session.clone(), tool, detail });
        }
    }
    transcript
}

/// Garde les transcripts déjà lus tant qu'ils ne changent pas.
#[derive(Default)]
pub struct Scanner {
    cache: HashMap<PathBuf, (SystemTime, u64, Arc<Transcript>)>,
}

impl Scanner {
    /// Transcripts d'un dossier de projet de Claude Code.
    pub fn scan(&mut self, dir: &Path) -> Vec<Arc<Transcript>> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let stamp = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
            let transcript = match self.cache.get(&path) {
                Some((modified, len, transcript)) if (*modified, *len) == stamp => transcript.clone(),
                _ => {
                    let transcript = Arc::new(read_transcript(&path));
                    self.cache.insert(path, (stamp.0, stamp.1, transcript.clone()));
                    transcript
                }
            };
            out.push(transcript);
        }
        out
    }
}

/// Totaux d'une période.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Totals {
    pub today: f64,
    pub month: f64,
    /// Ce mois, par modèle, du plus coûteux au moins coûteux.
    pub by_model: Vec<(String, f64)>,
}

/// `items` : (horodatage ISO, modèle, coût). `day` = « AAAA-MM-JJ », `month` = « AAAA-MM ».
pub fn totals<'a>(items: impl IntoIterator<Item = (&'a str, &'a str, f64)>, day: &str, month: &str) -> Totals {
    let mut totals = Totals::default();
    let mut models: HashMap<String, f64> = HashMap::new();
    for (time, model, cost) in items {
        if time.starts_with(month) {
            totals.month += cost;
            *models.entry(model.to_owned()).or_default() += cost;
            if time.starts_with(day) {
                totals.today += cost;
            }
        }
    }
    totals.by_model = models.into_iter().collect();
    totals.by_model.sort_by(|a, b| b.1.total_cmp(&a.1));
    totals
}

#[cfg(test)]
mod tests {
    use super::{cost, read_transcript, totals};
    use serde_json::json;

    #[test]
    fn prix() {
        let usage = json!({ "input_tokens": 1_000_000, "output_tokens": 100_000, "cache_read_input_tokens": 1_000_000 });
        // 4 $ + 2 $ + 0,20 $
        assert!((cost("claude-opus-5-5", &usage).unwrap() - 6.2).abs() < 1e-9);
        // Sonnet 5 ne doit pas prendre le prix de Sonnet 5.5 par erreur, et inversement.
        assert!((cost("claude-sonnet-5-5", &json!({ "output_tokens": 1_000_000 })).unwrap() - 10.0).abs() < 1e-9);
        let cached = json!({ "cache_creation_input_tokens": 1_000_000, "cache_creation": { "ephemeral_1h_input_tokens": 1_000_000, "ephemeral_5m_input_tokens": 0 } });
        assert!((cost("claude-haiku-5-5", &cached).unwrap() - 0.2).abs() < 1e-9);
        assert_eq!(cost("gpt-5", &usage), None);
    }

    #[test]
    fn transcript() {
        let dir = std::env::temp_dir().join(format!("supacode-costs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.jsonl");
        let usage = json!({ "input_tokens": 0, "output_tokens": 1_000_000 });
        let line = |content: serde_json::Value| {
            json!({ "type": "assistant", "timestamp": "2026-10-28T09:00:00Z", "sessionId": "s", "message": { "id": "m1", "model": "claude-sonnet-5-5", "usage": usage, "content": content } }).to_string()
        };
        let text = [line(json!([{ "type": "text", "text": "ok" }])), line(json!([{ "type": "tool_use", "name": "Bash", "input": { "command": "npm test" } }]))].join("\n");
        std::fs::write(&file, text).unwrap();
        let t = read_transcript(&file);
        assert_eq!(t.spends.len(), 1, "même réponse sur deux lignes");
        assert_eq!((t.activity[0].tool.as_str(), t.activity[0].detail.as_str()), ("Bash", "npm test"));
        let totals = totals(t.spends.iter().map(|s| (s.time.as_str(), s.model.as_str(), s.cost)), "2026-10-28", "2026-10");
        assert!((totals.today - 10.0).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(dir);
    }
}
