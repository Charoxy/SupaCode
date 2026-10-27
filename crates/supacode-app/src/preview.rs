//! Aperçu du code : surveille le projet de la conversation active et calcule,
//! pour chaque fichier modifié, le diff par rapport au dernier commit.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};
use similar::{ChangeTag, TextDiff};

/// Dossiers jamais affichés en aperçu.
const IGNORED: &[&str] = &[".git", "node_modules", "target", "dist", "out", "build", ".next", ".turbo", "coverage"];
/// Au-delà, le fichier n'est pas affiché (binaire ou trop gros).
const MAX_BYTES: u64 = 1_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineKind {
    Same,
    Added,
    Removed,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Numéro dans la version actuelle (absent pour une ligne supprimée).
    pub number: Option<usize>,
    pub text: String,
}

/// Un fichier modifié pendant la conversation.
#[derive(Clone, Debug)]
pub struct ChangedFile {
    pub path: PathBuf,
    /// Chemin relatif au projet, avec des `/`.
    pub relative: String,
    pub lines: Vec<DiffLine>,
    pub added: usize,
    pub removed: usize,
    /// Le fichier n'existe pas dans le dernier commit.
    pub is_new: bool,
}

impl ChangedFile {
    pub fn extension(&self) -> String {
        self.path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default()
    }
}

/// Surveille un dossier ; les chemins modifiés arrivent sur le canal.
pub fn watch(root: &Path) -> notify::Result<(notify::RecommendedWatcher, async_channel::Receiver<PathBuf>)> {
    let (tx, rx) = async_channel::unbounded();
    let root_owned = root.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else { return };
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            return;
        }
        for path in event.paths {
            if is_ignored(&root_owned, &path) || !path.is_file() {
                continue;
            }
            let _ = tx.try_send(path);
        }
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok((watcher, rx))
}

/// Délai de regroupement des écritures successives d'un même fichier.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

fn is_ignored(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else { return true };
    relative.components().any(|c| IGNORED.iter().any(|name| c.as_os_str() == *name))
}

/// Diff du fichier par rapport à `HEAD` (tout est « ajouté » si le fichier est nouveau).
pub fn diff_file(root: &Path, path: &Path) -> Option<ChangedFile> {
    if std::fs::metadata(path).ok()?.len() > MAX_BYTES {
        return None;
    }
    let current = std::fs::read_to_string(path).ok()?;
    let relative = path.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
    let committed = git_show_head(root, &relative);
    let is_new = committed.is_none();
    let old = committed.unwrap_or_default();

    let diff = TextDiff::from_lines(&old, &current);
    let (mut lines, mut added, mut removed) = (Vec::new(), 0, 0);
    for change in diff.iter_all_changes() {
        let kind = match change.tag() {
            ChangeTag::Equal => LineKind::Same,
            ChangeTag::Insert => {
                added += 1;
                LineKind::Added
            }
            ChangeTag::Delete => {
                removed += 1;
                LineKind::Removed
            }
        };
        let number = change.new_index().map(|i| i + 1);
        lines.push(DiffLine { kind, number, text: change.value().trim_end_matches(['\n', '\r']).to_owned() });
    }
    Some(ChangedFile { path: path.to_path_buf(), relative, lines, added, removed, is_new })
}

fn git_show_head(root: &Path, relative: &str) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(root).arg("show").arg(format!("HEAD:{relative}")).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Annule la modification : remet la version de `HEAD`, ou supprime un fichier nouveau.
pub fn revert(root: &Path, file: &ChangedFile) -> std::io::Result<()> {
    if file.is_new {
        return std::fs::remove_file(&file.path);
    }
    match git_show_head(root, &file.relative) {
        Some(content) => std::fs::write(&file.path, content),
        None => Err(std::io::Error::other("version du dernier commit introuvable")),
    }
}

/// Branche git courante du projet.
pub fn git_branch(root: &Path) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(root).args(["rev-parse", "--abbrev-ref", "HEAD"]).output().ok()?;
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !branch.is_empty()).then_some(branch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_d_un_fichier_nouveau_hors_git() {
        let dir = std::env::temp_dir().join(format!("supacode-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.ts");
        std::fs::write(&file, "const a = 1;\nconst b = 2;\n").unwrap();
        let changed = diff_file(&dir, &file).unwrap();
        assert!(changed.is_new);
        assert_eq!((changed.added, changed.removed), (2, 0));
        assert_eq!(changed.relative, "a.ts");
        assert!(is_ignored(&dir, &dir.join("node_modules").join("x.js")));
        assert!(!is_ignored(&dir, &file));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
