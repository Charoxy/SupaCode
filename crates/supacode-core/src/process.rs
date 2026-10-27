//! Lancement de processus : résolution des scripts `.cmd` sous Windows, lecture ligne à ligne, arrêt.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::claude::CLAUDE_SESSION_ENV;

/// Programme et arguments à lancer ; sous Windows, un script `.cmd`/`.bat` passe par `cmd.exe /c`.
pub fn resolve_program(program: &str, args: Vec<String>) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        if let Some(path) = find_in_path(program) {
            let is_script = path.extension().is_some_and(|x| x.eq_ignore_ascii_case("cmd") || x.eq_ignore_ascii_case("bat"));
            if is_script {
                let mut all = vec!["/c".to_owned(), path.display().to_string()];
                all.extend(args);
                return ("cmd.exe".into(), all);
            }
            return (path.display().to_string(), args);
        }
    }
    (program.to_owned(), args)
}

/// Chemin complet d'un programme du PATH (avec `.exe`, `.cmd` ou `.bat` sous Windows).
pub fn find_in_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions: &[&str] = if cfg!(windows) { &["exe", "cmd", "bat"] } else { &[""] };
    for dir in std::env::split_paths(&path) {
        for ext in extensions {
            let candidate = if ext.is_empty() { dir.join(program) } else { dir.join(format!("{program}.{ext}")) };
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Ce qu'il faut lancer.
pub struct Spawn<'a> {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: &'a Path,
    pub env: HashMap<String, String>,
    /// Envoyé sur l'entrée standard, qui est ensuite fermée.
    pub stdin: Option<String>,
}

impl<'a> Spawn<'a> {
    /// Commande passée au shell du système (`cmd.exe /c` ou `sh -c`).
    pub fn shell(command: &str, cwd: &'a Path) -> Self {
        let (program, args) = if cfg!(windows) {
            ("cmd.exe".to_owned(), vec!["/d".to_owned(), "/s".to_owned(), "/c".to_owned(), command.to_owned()])
        } else {
            ("sh".to_owned(), vec!["-c".to_owned(), command.to_owned()])
        };
        Self { program, args, cwd, env: HashMap::new(), stdin: None }
    }

    pub fn program(program: &str, args: Vec<String>, cwd: &'a Path) -> Self {
        let (program, args) = resolve_program(program, args);
        Self { program, args, cwd, env: HashMap::new(), stdin: None }
    }
}

/// Ligne de sortie d'un processus.
pub struct Line {
    pub text: String,
    pub stderr: bool,
}

/// Lance le processus, transmet chaque ligne (stdout et stderr) à `on_line` et renvoie le code de sortie.
/// Si `cancel` passe à vrai, le processus et ses enfants sont arrêtés (code `None`).
pub fn run_lines(spawn: Spawn, cancel: &AtomicBool, mut on_line: impl FnMut(Line)) -> Result<Option<i32>> {
    let mut command = Command::new(&spawn.program);
    command
        .args(&spawn.args)
        .current_dir(spawn.cwd)
        .envs(&spawn.env)
        .stdin(if spawn.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in CLAUDE_SESSION_ENV {
        command.env_remove(key);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Application graphique : pas de fenêtre de console pour chaque commande.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().with_context(|| format!("impossible de lancer {}", spawn.program))?;

    if let (Some(input), Some(mut stdin)) = (spawn.stdin, child.stdin.take()) {
        thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
    }
    let (tx, rx) = mpsc::channel();
    let readers: Vec<_> = [child.stdout.take().map(|s| (Box::new(s) as Box<dyn std::io::Read + Send>, false)), child.stderr.take().map(|s| (Box::new(s) as Box<dyn std::io::Read + Send>, true))]
        .into_iter()
        .flatten()
        .map(|(stream, stderr)| {
            let tx = tx.clone();
            thread::spawn(move || {
                for line in BufReader::new(stream).split(b'\n').map_while(Result::ok) {
                    let text = String::from_utf8_lossy(&line).trim_end_matches('\r').to_owned();
                    if tx.send(Line { text, stderr }).is_err() {
                        break;
                    }
                }
            })
        })
        .collect();
    drop(tx);

    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => on_line(line),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if cancel.load(Ordering::Relaxed) {
            kill_tree(&mut child);
            for reader in readers {
                let _ = reader.join();
            }
            return Ok(None);
        }
    }
    let status = child.wait()?;
    Ok(Some(status.code().unwrap_or(1)))
}

/// Arrête le processus et ses descendants (`cmd /c` laisse sinon tourner la vraie commande).
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lit_les_lignes_et_le_code_de_sortie() {
        let cancel = AtomicBool::new(false);
        let mut lines = Vec::new();
        let dir = std::env::temp_dir();
        let code = run_lines(Spawn::shell("echo un&& echo deux&& exit 3", &dir), &cancel, |l| lines.push(l.text)).unwrap();
        let lines: Vec<_> = lines.iter().map(|l| l.trim()).collect();
        assert_eq!(lines, ["un", "deux"]);
        assert_eq!(code, Some(3));
    }
}
