//! Conversations de Claude Code (`~/.claude/projects/<projet>/<session>.jsonl`) :
//! la liste des projets et de leurs conversations, à reprendre avec `claude --resume <id>`.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

/// Un projet connu de Claude Code.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    /// Nom du dossier dans `~/.claude/projects` (chemin encodé par Claude Code).
    pub id: String,
    /// Dossier du projet sur le disque.
    pub path: PathBuf,
    pub name: String,
    pub last_activity: SystemTime,
    pub conversation_count: usize,
}

/// Une conversation, reprise avec `claude --resume <id>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub last_activity: SystemTime,
}

/// Variables qui identifient une session Claude Code en cours. Si SupaCode est lancé depuis
/// Claude Code, ses enfants en héritent : un `claude` lancé ensuite se croirait sous-session
/// (historique non enregistré…). On les retire. Les réglages (ex. `CLAUDE_CODE_USE_BEDROCK`) restent.
pub const CLAUDE_SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SSE_PORT",
];

/// Octets lus au début et à la fin de chaque fichier : le dossier et les titres s'y trouvent.
const HEAD_BYTES: u64 = 64 * 1024;
const TAIL_BYTES: u64 = 256 * 1024;

/// `~/.claude/projects` (ou `$CLAUDE_CONFIG_DIR/projects`).
pub fn projects_dir() -> Option<PathBuf> {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))?;
    Some(base.join("projects"))
}

/// Projets ayant au moins une conversation et dont le dossier existe encore, du plus récent au plus ancien.
pub fn list_projects(projects_dir: &Path) -> Vec<Project> {
    let Ok(entries) = fs::read_dir(projects_dir) else {
        return Vec::new();
    };
    let mut projects: Vec<Project> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().into_owned();
            let sessions = session_files(&entry.path());
            let latest = sessions.first()?;
            // Une session vide n'a pas de dossier : on regarde les suivantes.
            let cwd = sessions.iter().take(5).find_map(|s| read_session(&s.0).cwd);
            let path = cwd.map(PathBuf::from).unwrap_or_else(|| decode_project_id(&id));
            if !path.exists() {
                return None;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            Some(Project { id, path, name, last_activity: latest.1, conversation_count: sessions.len() })
        })
        .collect();
    projects.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    projects
}

/// Conversations d'un projet, de la plus récente à la plus ancienne (les vides sont ignorées).
pub fn list_conversations(projects_dir: &Path, project_id: &str, limit: usize) -> Vec<Conversation> {
    session_files(&projects_dir.join(project_id))
        .into_iter()
        .take(limit)
        .filter_map(|(file, modified)| {
            let title = read_session(&file).title?;
            let id = file.file_stem()?.to_string_lossy().into_owned();
            Some(Conversation { id, project_id: project_id.to_owned(), title, last_activity: modified })
        })
        .collect()
}

/// Fichiers `.jsonl` non vides, du plus récent au plus ancien.
fn session_files(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, SystemTime)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            (meta.len() > 0).then(|| (e.path(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .collect();
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files
}

#[derive(Debug, Default, PartialEq)]
pub struct SessionInfo {
    pub cwd: Option<String>,
    pub title: Option<String>,
}

fn read_session(file: &Path) -> SessionInfo {
    let Ok(mut handle) = File::open(file) else {
        return SessionInfo::default();
    };
    let size = handle.metadata().map(|m| m.len()).unwrap_or(0);
    let mut text = Vec::new();
    let _ = (&mut handle).take(HEAD_BYTES).read_to_end(&mut text);
    if size > HEAD_BYTES {
        let length = TAIL_BYTES.min(size - HEAD_BYTES);
        if handle.seek(SeekFrom::Start(size - length)).is_ok() {
            text.push(b'\n');
            let _ = handle.take(length).read_to_end(&mut text);
        }
    }
    parse_session_text(&String::from_utf8_lossy(&text))
}

/// Titre par ordre de préférence : titre donné par l'utilisateur (`/rename`), titre généré,
/// dernière demande, sinon premier message de l'utilisateur.
pub fn parse_session_text(text: &str) -> SessionInfo {
    let (mut cwd, mut custom, mut ai, mut last_prompt, mut first_user) = (None, None, None, None, None);
    for line in text.lines() {
        // Filtre rapide : on ne parse que les lignes utiles (les fichiers peuvent être gros).
        if !line.starts_with('{')
            || !["\"cwd\"", "customTitle", "aiTitle", "lastPrompt", "\"type\":\"user\""].iter().any(|k| line.contains(k))
        {
            continue;
        }
        // Une ligne coupée par la lecture partielle ne se parse pas : on l'ignore.
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        let str_field = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
        if cwd.is_none() {
            cwd = str_field("cwd");
        }
        match entry.get("type").and_then(Value::as_str) {
            Some("custom-title") => custom = str_field("customTitle").or(custom),
            Some("ai-title") => ai = str_field("aiTitle").or(ai),
            Some("last-prompt") => last_prompt = str_field("lastPrompt").or(last_prompt),
            Some("user") if first_user.is_none() => {
                let internal = entry.get("isMeta").and_then(Value::as_bool).unwrap_or(false)
                    || entry.get("isSidechain").and_then(Value::as_bool).unwrap_or(false);
                if !internal {
                    first_user = user_text(entry.get("message"));
                }
            }
            _ => {}
        }
    }
    let title = [custom, ai, last_prompt, first_user]
        .into_iter()
        .flatten()
        .map(|t| one_line(&t))
        .find(|t| !t.is_empty());
    SessionInfo { cwd, title }
}

fn user_text(message: Option<&Value>) -> Option<String> {
    let content = message?.get("content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .find(|b| b.get("type").and_then(Value::as_str) == Some("text"))?
            .get("text")?
            .as_str()?
            .to_owned(),
        _ => return None,
    };
    // Les commandes et sorties internes (<command-name>…) ne font pas un bon titre.
    (!text.trim_start().starts_with('<')).then_some(text)
}

fn one_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 80 { format!("{}…", line.chars().take(79).collect::<String>()) } else { line }
}

/// Repli si aucune session ne donne le dossier : « G--workspace-app » → « G:\workspace\app » (approximatif).
fn decode_project_id(id: &str) -> PathBuf {
    let bytes = id.as_bytes();
    if bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && &id[1..3] == "--" {
        return PathBuf::from(format!("{}:\\{}", &id[..1], id[3..].replace('-', "\\")));
    }
    PathBuf::from(id.replace('-', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn jsonl(entries: &[Value]) -> String {
        entries.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn titre_par_ordre_de_preference() {
        let user = json!({"type": "user", "cwd": "/p", "message": {"content": "corrige le bug"}});
        assert_eq!(parse_session_text(&jsonl(&[user.clone()])).title.as_deref(), Some("corrige le bug"));
        let last = json!({"type": "last-prompt", "lastPrompt": "ajoute un test"});
        assert_eq!(parse_session_text(&jsonl(&[user.clone(), last])).title.as_deref(), Some("ajoute un test"));
        let ai = json!({"type": "ai-title", "aiTitle": "Bug du panier"});
        assert_eq!(parse_session_text(&jsonl(&[user.clone(), ai.clone()])).title.as_deref(), Some("Bug du panier"));
        let custom = json!({"type": "custom-title", "customTitle": "Mon titre"});
        assert_eq!(parse_session_text(&jsonl(&[custom, ai])).title.as_deref(), Some("Mon titre"));
        assert_eq!(parse_session_text(&jsonl(&[user])).cwd.as_deref(), Some("/p"));
    }

    #[test]
    fn ignore_messages_internes_et_lignes_coupees() {
        let meta = json!({"type": "user", "isMeta": true, "message": {"content": "méta"}});
        let command = json!({"type": "user", "message": {"content": "<command-name>/clear</command-name>"}});
        let real = json!({"type": "user", "message": {"content": [{"type": "text", "text": "vrai message"}]}});
        let text = format!("{{\"type\":\"user\",\"cwd\":\"/coup\n{}", jsonl(&[meta, command, real]));
        assert_eq!(parse_session_text(&text).title.as_deref(), Some("vrai message"));
    }

    #[test]
    fn liste_projets_et_conversations() {
        let root = std::env::temp_dir().join(format!("supacode-claude-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let project_dir = root.join("projects").join("mon-projet");
        fs::create_dir_all(&project_dir).unwrap();
        let cwd = root.to_string_lossy().to_string();
        let user = |text: &str| json!({"type": "user", "cwd": cwd, "message": {"content": text}});
        fs::write(project_dir.join("ancienne.jsonl"), jsonl(&[user("première")])).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(project_dir.join("recente.jsonl"), jsonl(&[user("seconde")])).unwrap();
        fs::write(project_dir.join("vide.jsonl"), jsonl(&[json!({"type": "mode"})])).unwrap();
        // Projet dont le dossier n'existe plus : ignoré.
        let gone = root.join("projects").join("disparu");
        fs::create_dir_all(&gone).unwrap();
        fs::write(gone.join("s.jsonl"), jsonl(&[json!({"type": "user", "cwd": "/nexiste/pas", "message": {"content": "x"}})])).unwrap();

        let projects = list_projects(&root.join("projects"));
        assert_eq!(projects.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["mon-projet"]);
        assert_eq!(projects[0].path, root);
        assert_eq!(projects[0].conversation_count, 3);

        let titles: Vec<_> = list_conversations(&root.join("projects"), "mon-projet", 50).into_iter().map(|c| c.title).collect();
        assert_eq!(titles, ["seconde", "première"]);
        let _ = fs::remove_dir_all(&root);
    }
}
