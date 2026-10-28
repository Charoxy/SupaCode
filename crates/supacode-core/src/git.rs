//! Opérations git pour travailler avec les agents sans risque :
//! - points de retour : instantané de tout le dossier (fichiers non suivis compris, hors
//!   `.gitignore`) sans toucher à l'index ni aux branches, et retour à cet état ;
//! - worktrees : une copie de travail par tâche, son diff, puis l'application au projet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context as _, Result, anyhow, bail};

use crate::process::{self, Spawn};

/// Lance git ; renvoie la sortie standard si le code est 0.
pub fn git(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String> {
    let mut spawn = Spawn::program("git", args.iter().map(|a| (*a).to_owned()).collect(), root);
    spawn.env = env.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect::<HashMap<_, _>>();
    let (mut out, mut err) = (String::new(), String::new());
    let code = process::run_lines(spawn, &AtomicBool::new(false), |line| {
        let target = if line.stderr { &mut err } else { &mut out };
        target.push_str(&line.text);
        target.push('\n');
    })?;
    if code != Some(0) {
        bail!("git {} : {}", args.first().unwrap_or(&""), err.trim());
    }
    Ok(out)
}

fn git_dir(root: &Path) -> Result<PathBuf> {
    let dir = git(root, &["rev-parse", "--absolute-git-dir"], &[])?;
    Ok(PathBuf::from(dir.trim()))
}

// — Points de retour

/// Point de retour : un commit hors branche qui contient tout le dossier de travail.
#[derive(Debug, Clone, PartialEq)]
pub struct Checkpoint {
    pub commit: String,
    pub label: String,
    /// Horodatage ISO (UTC).
    pub time: String,
}

/// Enregistre l'état du dossier dans `refs/supacode/checkpoints/<groupe>/<n>` ; l'index et
/// les branches de l'utilisateur ne bougent pas.
pub fn checkpoint(root: &Path, group: &str, label: &str) -> Result<Checkpoint> {
    let index = git_dir(root)?.join(format!("supacode-index-{}", std::process::id()));
    let index_str = index.display().to_string();
    let env = [("GIT_INDEX_FILE", index_str.as_str())];
    let result = (|| {
        git(root, &["add", "-A"], &env)?;
        let tree = git(root, &["write-tree"], &env)?.trim().to_owned();
        // Rien n'a changé depuis le dernier point : on le garde (les validations de Claude
        // passent aussi par Entrée).
        if let Some(last) = checkpoints(root, group).into_iter().next() {
            if git(root, &["rev-parse", &format!("{}^{{tree}}", last.commit)], &[]).is_ok_and(|t| t.trim() == tree) {
                return Ok(last);
            }
        }
        let has_head = git(root, &["rev-parse", "--verify", "-q", "HEAD"], &[]).is_ok();
        let message = format!("SupaCode : {label}");
        let commit = if has_head { git(root, &["commit-tree", &tree, "-p", "HEAD", "-m", &message], &[])? } else { git(root, &["commit-tree", &tree, "-m", &message], &[])? }.trim().to_owned();
        let time = crate::audit::now();
        // Deux points dans la même seconde : le début du commit les distingue.
        let reference = format!("refs/supacode/checkpoints/{}/{}-{}", sanitize(group), time.replace([':', '-'], ""), &commit[..commit.len().min(8)]);
        git(root, &["update-ref", &reference, &commit], &[])?;
        Ok(Checkpoint { commit, label: label.to_owned(), time })
    })();
    let _ = std::fs::remove_file(&index);
    result
}

fn sanitize(text: &str) -> String {
    text.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

/// Points de retour d'un groupe (une conversation), du plus récent au plus ancien.
pub fn checkpoints(root: &Path, group: &str) -> Vec<Checkpoint> {
    let prefix = format!("refs/supacode/checkpoints/{}/", sanitize(group));
    let Ok(out) = git(root, &["for-each-ref", "--sort=-refname", "--format=%(objectname)\x1f%(refname)\x1f%(contents:subject)", &prefix], &[]) else { return Vec::new() };
    out.lines()
        .filter_map(|line| {
            let mut parts = line.split('\x1f');
            let commit = parts.next()?.to_owned();
            let stamp = parts.next()?.rsplit('/').next()?.to_owned();
            let label = parts.next().unwrap_or("").trim_start_matches("SupaCode : ").to_owned();
            // 20261028T101500Z → 2026-10-28T10:15:00Z
            let stamp = &stamp[..stamp.len().min(16)];
            let time = if stamp.len() == 16 { format!("{}-{}-{}T{}:{}:{}Z", &stamp[0..4], &stamp[4..6], &stamp[6..8], &stamp[9..11], &stamp[11..13], &stamp[13..15]) } else { stamp.to_owned() };
            Some(Checkpoint { commit, label, time })
        })
        .collect()
}

/// Fichiers modifiés entre un point de retour et l'état actuel.
pub fn changed_since(root: &Path, commit: &str) -> Result<Vec<String>> {
    let current = checkpoint_tree(root)?;
    let out = git(root, &["diff", "--name-only", commit, &current], &[])?;
    Ok(out.lines().map(str::to_owned).collect())
}

/// Arbre de l'état actuel (sans rien enregistrer).
fn checkpoint_tree(root: &Path) -> Result<String> {
    let index = git_dir(root)?.join(format!("supacode-index-tree-{}", std::process::id()));
    let index_str = index.display().to_string();
    let env = [("GIT_INDEX_FILE", index_str.as_str())];
    let tree = git(root, &["add", "-A"], &env).and_then(|_| git(root, &["write-tree"], &env)).map(|t| t.trim().to_owned());
    let _ = std::fs::remove_file(&index);
    tree
}

/// Ramène le dossier à l'état d'un point de retour : fichiers modifiés ou supprimés depuis
/// restaurés, fichiers créés depuis supprimés. L'état actuel est d'abord enregistré
/// (« avant retour »), pour pouvoir annuler.
pub fn restore(root: &Path, group: &str, commit: &str) -> Result<Checkpoint> {
    let safety = checkpoint(root, group, "avant retour en arrière")?;
    let added = git(root, &["diff", "--name-only", "--diff-filter=A", commit, &safety.commit], &[])?;
    for file in added.lines().filter(|l| !l.is_empty()) {
        let _ = std::fs::remove_file(root.join(file));
    }
    // Seuls les fichiers modifiés ou supprimés depuis le point sont réécrits : les autres
    // ne bougent pas (pas de conversion de fins de ligne inutile).
    let touched = git(root, &["diff", "--name-only", "--diff-filter=MDT", commit, &safety.commit], &[])?;
    let files: Vec<&str> = touched.lines().filter(|l| !l.is_empty()).collect();
    if files.is_empty() {
        return Ok(safety);
    }
    let index = git_dir(root)?.join(format!("supacode-index-restore-{}", std::process::id()));
    let index_str = index.display().to_string();
    let env = [("GIT_INDEX_FILE", index_str.as_str())];
    let mut args = vec!["checkout-index", "-f", "--"];
    args.extend(files.iter().copied());
    let result = git(root, &["read-tree", commit], &env).and_then(|_| git(root, &args, &env));
    let _ = std::fs::remove_file(&index);
    result.context("restauration des fichiers")?;
    Ok(safety)
}

// — Worktrees

/// Crée une copie de travail sur une nouvelle branche, à partir de HEAD.
pub fn add_worktree(root: &Path, name: &str) -> Result<(PathBuf, String)> {
    git(root, &["rev-parse", "--verify", "HEAD"], &[]).map_err(|_| anyhow!("le projet n'a pas encore de commit : committe une première fois pour lancer des tâches"))?;
    let dir = root.join(".supacode").join("worktrees");
    std::fs::create_dir_all(&dir)?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(ignore, "*\n")?;
    }
    let base = sanitize(name);
    let (path, branch) = (1..)
        .map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
        .map(|n| (dir.join(&n), format!("supacode/{n}")))
        .find(|(p, b)| !p.exists() && git(root, &["rev-parse", "--verify", "-q", &format!("refs/heads/{b}")], &[]).is_err())
        .ok_or_else(|| anyhow!("nom de worktree indisponible"))?;
    git(root, &["worktree", "add", "-b", &branch, &path.display().to_string(), "HEAD"], &[])?;
    Ok((path, branch))
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileDiff {
    pub path: String,
    pub added: usize,
    pub removed: usize,
}

/// Fichiers modifiés dans un worktree depuis son point de départ (fichiers nouveaux compris).
pub fn worktree_diff(worktree: &Path) -> Result<Vec<FileDiff>> {
    git(worktree, &["add", "-A", "-N"], &[])?;
    let out = git(worktree, &["diff", "--numstat", "HEAD"], &[])?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let added = parts.next()?.parse().unwrap_or(0);
            let removed = parts.next()?.parse().unwrap_or(0);
            Some(FileDiff { path: parts.next()?.to_owned(), added, removed })
        })
        .collect())
}

/// Diff complet d'un fichier du worktree (format unifié).
pub fn worktree_file_diff(worktree: &Path, file: &str) -> Result<String> {
    git(worktree, &["add", "-A", "-N"], &[])?;
    git(worktree, &["diff", "HEAD", "--", file], &[])
}

/// Applique au projet les changements du worktree (sans commit) ; refuse en cas de conflit.
pub fn apply_worktree(root: &Path, worktree: &Path) -> Result<usize> {
    git(worktree, &["add", "-A"], &[])?;
    let patch = git(worktree, &["diff", "--cached", "--binary", "HEAD"], &[])?;
    if patch.trim().is_empty() {
        bail!("cette tâche n'a rien modifié");
    }
    let file = git_dir(root)?.join(format!("supacode-patch-{}.diff", std::process::id()));
    std::fs::write(&file, &patch)?;
    let path = file.display().to_string();
    // Sans toucher à l'index (l'utilisateur relit avant de committer) ; en cas de
    // divergence, fusion à trois voies.
    let applied = git(root, &["apply", "--whitespace=nowarn", &path], &[]).or_else(|_| git(root, &["apply", "--3way", "--whitespace=nowarn", &path], &[]));
    let _ = std::fs::remove_file(&file);
    applied.context("les changements de la tâche entrent en conflit avec ton dossier : applique-les à la main depuis le worktree")?;
    Ok(patch.matches("\ndiff --git").count() + usize::from(patch.starts_with("diff --git")))
}

/// Supprime le worktree et sa branche.
pub fn remove_worktree(root: &Path, worktree: &Path, branch: &str) -> Result<()> {
    git(root, &["worktree", "remove", "--force", &worktree.display().to_string()], &[])?;
    let _ = git(root, &["branch", "-D", branch], &[]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        let root = std::env::temp_dir().join(format!("supacode-git-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&root).unwrap();
        for args in [&["init", "-q"][..], &["config", "user.email", "t@t"], &["config", "user.name", "t"]] {
            git(&root, args, &[]).unwrap();
        }
        std::fs::write(root.join("a.txt"), "un\n").unwrap();
        git(&root, &["add", "-A"], &[]).unwrap();
        git(&root, &["commit", "-qm", "init"], &[]).unwrap();
        root
    }

    #[test]
    fn retour_en_arriere() {
        let root = repo();
        std::fs::write(root.join("a.txt"), "deux\n").unwrap();
        let point = checkpoint(&root, "conv", "avant l'agent").unwrap();
        assert_eq!(checkpoint(&root, "conv", "doublon").unwrap(), point, "rien n'a changé : même point");
        // L'agent modifie, crée et supprime.
        std::fs::write(root.join("a.txt"), "trois\n").unwrap();
        std::fs::write(root.join("nouveau.txt"), "x\n").unwrap();
        assert_eq!(changed_since(&root, &point.commit).unwrap().len(), 2);
        restore(&root, "conv", &point.commit).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap().replace("\r\n", "\n"), "deux\n");
        assert!(!root.join("nouveau.txt").exists());
        // L'index de l'utilisateur n'a pas bougé, et l'état d'avant le retour est gardé.
        assert!(git(&root, &["diff", "--cached", "--name-only"], &[]).unwrap().trim().is_empty());
        let list = checkpoints(&root, "conv");
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|c| c.label == "avant l'agent"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn worktree_puis_application() {
        let root = repo();
        let (wt, branch) = add_worktree(&root, "tâche 1").unwrap();
        std::fs::write(wt.join("a.txt"), "modifié\n").unwrap();
        std::fs::write(wt.join("b.txt"), "nouveau\n").unwrap();
        let diff = worktree_diff(&wt).unwrap();
        assert_eq!(diff.len(), 2);
        apply_worktree(&root, &wt).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap().replace("\r\n", "\n"), "nouveau\n");
        remove_worktree(&root, &wt, &branch).unwrap();
        assert!(!wt.exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
