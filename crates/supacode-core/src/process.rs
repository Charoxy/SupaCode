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
    /// Sous Windows : fin de ligne de commande transmise telle quelle (voir [`Spawn::shell`]).
    #[cfg_attr(not(windows), allow(dead_code))]
    raw: Option<String>,
}

impl<'a> Spawn<'a> {
    /// Commande passée au shell du système (`cmd.exe /c` ou `sh -c`).
    pub fn shell(command: &str, cwd: &'a Path) -> Self {
        if cfg!(windows) {
            // `cmd.exe` ne comprend pas l'échappement `\"` qu'appliquerait Rust : la commande est
            // passée brute, entre guillemets que `/s` retire.
            let args = ["/d", "/s", "/c"].map(Into::into).into();
            Self { program: "cmd.exe".into(), args, cwd, env: HashMap::new(), stdin: None, raw: Some(format!("\"{command}\"")) }
        } else {
            Self { program: "sh".into(), args: vec!["-c".into(), command.into()], cwd, env: HashMap::new(), stdin: None, raw: None }
        }
    }

    pub fn program(program: &str, args: Vec<String>, cwd: &'a Path) -> Self {
        let (program, args) = resolve_program(program, args);
        Self { program, args, cwd, env: HashMap::new(), stdin: None, raw: None }
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
        if let Some(raw) = &spawn.raw {
            command.raw_arg(raw);
        }
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
                    let text = decode(&line).trim_end_matches('\r').to_owned();
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

/// Texte d'une ligne de sortie : UTF-8, sinon (Windows) la page de code OEM de la console,
/// celle qu'utilisent `cmd.exe` et ses commandes internes.
fn decode(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_owned();
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MultiByteToWideChar(code_page: u32, flags: u32, text: *const u8, len: i32, wide: *mut u16, wide_len: i32) -> i32;
        }
        const CP_OEMCP: u32 = 1;
        let len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
        // SAFETY : tampons valides et longueurs exactes ; le premier appel ne fait que mesurer.
        let needed = unsafe { MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0) };
        if needed > 0 {
            let mut wide = vec![0u16; needed as usize];
            let written = unsafe { MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), needed) };
            if written > 0 {
                return String::from_utf16_lossy(&wide[..written as usize]);
            }
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
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

    #[test]
    fn garde_les_guillemets_de_la_commande() {
        let cancel = AtomicBool::new(false);
        let dir = std::env::temp_dir().join(format!("supacode-quotes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a b.txt"), "contenu").unwrap();
        let command = if cfg!(windows) { r#"type "a b.txt""# } else { r#"cat "a b.txt""# };
        let mut lines = Vec::new();
        let code = run_lines(Spawn::shell(command, &dir), &cancel, |l| lines.push(l.text)).unwrap();
        assert_eq!((code, lines), (Some(0), vec!["contenu".to_owned()]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
