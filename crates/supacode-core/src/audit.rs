//! Journal d'audit : ce que SupaCode et les agents ont fait dans un projet, qui l'a lancé
//! et ce que ça a coûté. Une ligne JSON par événement, dans
//! `.supacode/journal/AAAA-MM.jsonl` (ignoré par git : il contient les demandes).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::process::{self, Spawn};

pub mod kind {
    pub const CONVERSATION: &str = "conversation";
    pub const WORKFLOW: &str = "workflow";
    pub const COMMAND: &str = "commande";
    pub const AGENT: &str = "agent";
    pub const TOOL: &str = "outil";
    pub const APPROVAL: &str = "validation";
    pub const BLOCKED: &str = "blocage";
    pub const BROWSER: &str = "navigateur";
    pub const POLICY: &str = "règles";
    pub const PROMPT: &str = "prompt";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// ISO 8601, UTC.
    pub time: String,
    pub user: String,
    pub kind: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub details: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

fn dir(root: &Path) -> PathBuf {
    root.join(".supacode").join("journal")
}

/// Ajoute un événement au journal du projet (les erreurs d'écriture sont ignorées :
/// le journal ne doit jamais empêcher le travail).
pub fn record(root: &Path, kind: &str, summary: impl Into<String>, details: Value, cost_usd: Option<f64>) {
    let entry = Entry { time: now(), user: user(root), kind: kind.to_owned(), summary: summary.into(), details, cost_usd };
    let dir = dir(root);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        let _ = std::fs::write(ignore, "*\n");
    }
    let file = dir.join(format!("{}.jsonl", &entry.time[..7]));
    if let (Ok(mut out), Ok(line)) = (std::fs::OpenOptions::new().create(true).append(true).open(file), serde_json::to_string(&entry)) {
        let _ = writeln!(out, "{line}");
    }
}

/// Tout le journal du projet, du plus ancien au plus récent.
pub fn read(root: &Path) -> Vec<Entry> {
    let Ok(files) = std::fs::read_dir(dir(root)) else { return Vec::new() };
    let mut files: Vec<_> = files.flatten().map(|f| f.path()).filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl")).collect();
    files.sort();
    let mut entries: Vec<Entry> = files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .flat_map(|text| text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect::<Vec<_>>())
        .collect();
    entries.sort_by(|a, b| a.time.cmp(&b.time));
    entries
}

/// Export tableur (séparateur `;`, lisible par Excel en français).
pub fn to_csv(entries: &[Entry]) -> String {
    let field = |s: &str| format!("\"{}\"", s.replace('"', "\"\"").replace(['\n', '\r'], " "));
    let mut out = String::from("\u{feff}date;utilisateur;type;résumé;coût estimé (USD);détails\n");
    for e in entries {
        let cost = e.cost_usd.map(|c| format!("{c:.4}").replace('.', ",")).unwrap_or_default();
        let details = if e.details.is_null() { String::new() } else { e.details.to_string() };
        out.push_str(&[field(&e.time), field(&e.user), field(&e.kind), field(&e.summary), cost, field(&details)].join(";"));
        out.push('\n');
    }
    out
}

/// Utilisateur git du projet (`Nom <e-mail>`), ou celui de la session.
pub fn user(root: &Path) -> String {
    static USERS: Mutex<Option<HashMap<PathBuf, String>>> = Mutex::new(None);
    if let Some(user) = USERS.lock().unwrap().get_or_insert_with(HashMap::new).get(root) {
        return user.clone();
    }
    let user = {
        let git = |key: &str| {
            let mut value = String::new();
            // Dans le projet : son identité locale passe avant la globale.
            let cwd = if root.is_dir() { root.to_owned() } else { std::env::temp_dir() };
            let spawn = Spawn::program("git", vec!["config".into(), "--get".into(), key.into()], &cwd);
            let _ = process::run_lines(spawn, &Default::default(), |line| {
                if !line.stderr && value.is_empty() {
                    value = line.text.trim().to_owned();
                }
            });
            value
        };
        let (name, email) = (git("user.name"), git("user.email"));
        match (name.is_empty(), email.is_empty()) {
            (false, false) => format!("{name} <{email}>"),
            (false, true) => name,
            (true, false) => email,
            (true, true) => std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "inconnu".into()),
        }
    };
    USERS.lock().unwrap().get_or_insert_with(HashMap::new).insert(root.to_owned(), user.clone());
    user
}

/// Maintenant, en ISO 8601 UTC : « 2026-10-28T09:05:00Z ».
pub fn now() -> String {
    iso(SystemTime::now())
}

pub fn iso(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let rest = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// « 2026-10-28T09:05:00(.123)Z » → instant ; `None` si le format est autre.
pub fn parse(iso: &str) -> Option<SystemTime> {
    let n = |range: std::ops::Range<usize>| iso.get(range)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, mi, s) = (n(11..13)?, n(14..16)?, n(17..19)?);
    // Inverse de `civil` (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + s;
    u64::try_from(secs).ok().map(|s| UNIX_EPOCH + std::time::Duration::from_secs(s))
}

/// Jour depuis 1970 → (année, mois, jour) — algorithme de Howard Hinnant.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::{Entry, iso, read, record, to_csv};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn dates() {
        assert_eq!(iso(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(iso(UNIX_EPOCH + Duration::from_secs(1_793_181_600)), "2026-10-28T10:00:00Z");
        assert_eq!(iso(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_793_181_605);
        assert_eq!(super::parse(&iso(t)), Some(t));
        assert_eq!(super::parse("2026-10-28T10:00:05.123Z"), Some(t));
    }

    #[test]
    fn ecrit_et_relit() {
        let root = std::env::temp_dir().join(format!("supacode-audit-{}", std::process::id()));
        record(&root, "commande", "npm test", serde_json::json!({ "code": 0 }), None);
        record(&root, "agent", "Claude « corrige »", serde_json::Value::Null, Some(0.5));
        let entries: Vec<Entry> = read(&root);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].cost_usd, Some(0.5));
        assert!(to_csv(&entries).contains("\"npm test\""));
        assert!(root.join(".supacode/journal/.gitignore").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
