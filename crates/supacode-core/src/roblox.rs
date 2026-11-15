//! Outils Roblox : projets Rojo, état de Rojo et de Roblox Studio, outils installés
//! (Rojo, Blender) et skills livrés avec SupaCode (build, modèles 3D, Luau).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::process::find_in_path;

/// Port de `rojo serve` quand le projet n'en précise pas.
pub const DEFAULT_PORT: u16 = 34872;

/// Projet Roblox reconnu dans un dossier.
#[derive(Clone, Debug)]
pub struct RobloxProject {
    /// Nom du projet Rojo (champ `name`), sinon celui du dossier.
    pub name: String,
    pub root: PathBuf,
    /// Fichier `*.project.json` (le `default.project.json` d'abord).
    pub project_file: Option<PathBuf>,
    /// Fichiers de place (`.rbxl`, `.rbxlx`) à la racine ou dans `build/`.
    pub places: Vec<PathBuf>,
    /// Outils de la chaîne repérés : wally, rokit, aftman, selene, stylua…
    pub toolchain: Vec<&'static str>,
    pub serve_port: u16,
}

/// Reconnaît un projet Roblox : un projet Rojo ou, à défaut, un dossier avec une place.
pub fn detect(root: &Path) -> Option<RobloxProject> {
    let entries: Vec<PathBuf> = std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).collect();
    let name_of = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut project_files: Vec<&PathBuf> = entries.iter().filter(|p| name_of(p).ends_with(".project.json")).collect();
    project_files.sort_by_key(|p| (name_of(p) != "default.project.json", name_of(p)));
    let project_file = project_files.first().map(|p| (*p).clone());

    let is_place = |p: &Path| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rbxl") || x.eq_ignore_ascii_case("rbxlx"));
    let mut places: Vec<PathBuf> = entries.iter().filter(|p| is_place(p)).cloned().collect();
    if let Ok(build) = std::fs::read_dir(root.join("build")) {
        places.extend(build.flatten().map(|e| e.path()).filter(|p| is_place(p)));
    }
    if project_file.is_none() && places.is_empty() {
        return None;
    }

    let json = project_file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let name = json.as_ref().and_then(|j| j.get("name")).and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| name_of(root));
    let serve_port = json.as_ref().and_then(|j| j.get("servePort")).and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).unwrap_or(DEFAULT_PORT);
    let markers: [(&str, &'static str); 6] = [
        ("wally.toml", "wally"),
        ("rokit.toml", "rokit"),
        ("aftman.toml", "aftman"),
        ("foreman.toml", "foreman"),
        ("selene.toml", "selene"),
        ("stylua.toml", "stylua"),
    ];
    let toolchain = markers.iter().filter(|(file, _)| root.join(file).is_file()).map(|(_, tool)| *tool).collect();
    Some(RobloxProject { name, root: root.to_path_buf(), project_file, places, toolchain, serve_port })
}

/// Projet servi par `rojo serve` sur ce port (nom du projet), s'il y en a un.
pub fn rojo_serving(port: u16) -> Option<String> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(250)).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(800))).ok()?;
    write!(stream, "GET /api/rojo HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n").ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let body = response.split_once("\r\n\r\n")?.1;
    let json: Value = serde_json::from_str(body.trim()).ok()?;
    json.get("projectName").and_then(Value::as_str).map(str::to_owned)
}

/// Titres des fenêtres de Roblox Studio ouvertes (« Place - Roblox Studio »).
pub fn studio_windows() -> Vec<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = std::process::Command::new("tasklist")
            .args(["/v", "/fo", "csv", "/nh", "/fi", "imagename eq RobloxStudioBeta.exe"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        let Ok(output) = output else { return Vec::new() };
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines()
            .filter(|l| l.starts_with('"'))
            .filter_map(|l| l.trim().trim_end_matches('"').rsplit("\",\"").next().map(str::to_owned))
            .filter(|title| !title.is_empty() && title != "N/A" && !title.eq_ignore_ascii_case("Non disponible"))
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Le projet est-il ouvert dans une fenêtre de Studio (titre qui porte son nom ou celui d'une place) ?
pub fn open_in_studio(project: &RobloxProject, windows: &[String]) -> bool {
    let mut names = vec![project.name.to_lowercase()];
    names.extend(project.places.iter().filter_map(|p| p.file_stem()).map(|s| s.to_string_lossy().to_lowercase()));
    windows.iter().any(|title| {
        let title = title.to_lowercase();
        names.iter().any(|n| !n.is_empty() && title.contains(n.as_str()))
    })
}

/// Chemin de Rojo : dans le PATH, sinon dans les dossiers de Rokit, Aftman ou Foreman.
pub fn find_rojo() -> Option<PathBuf> {
    find_in_path("rojo").or_else(|| {
        let home = dirs::home_dir()?;
        let exe = if cfg!(windows) { "rojo.exe" } else { "rojo" };
        [".rokit", ".aftman", ".foreman"].iter().map(|d| home.join(d).join("bin").join(exe)).find(|p| p.is_file())
    })
}

/// Chemin de Blender : dans le PATH, sinon la version la plus récente installée.
pub fn find_blender() -> Option<PathBuf> {
    if let Some(path) = find_in_path("blender") {
        return Some(path);
    }
    #[cfg(windows)]
    {
        let base = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        let mut found: Vec<PathBuf> = std::fs::read_dir(base.join("Blender Foundation"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path().join("blender.exe"))
            .filter(|p| p.is_file())
            .collect();
        // « Blender 5.1 » après « Blender 4.2 » : la dernière est la plus récente.
        found.sort_by_key(|p| version_key(p));
        return found.pop();
    }
    #[cfg(not(windows))]
    None
}

#[cfg_attr(not(windows), allow(dead_code))]
fn version_key(path: &Path) -> Vec<u32> {
    let folder = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    folder.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse().ok()).collect()
}

/// Construit la place du projet avec Rojo ; renvoie le fichier produit.
pub fn build_place(project: &RobloxProject) -> Result<PathBuf> {
    let rojo = find_rojo().context("Rojo est introuvable (installe-le avec Rokit : rokit add rojo-rbx/rojo)")?;
    let project_file = project.project_file.as_ref().context("ce projet n'a pas de fichier *.project.json")?;
    let output = project.root.join("build").join(format!("{}.rbxlx", safe_name(&project.name)));
    std::fs::create_dir_all(output.parent().unwrap_or(&project.root))?;
    let mut command = std::process::Command::new(&rojo);
    command.arg("build").arg(project_file).arg("-o").arg(&output).current_dir(&project.root);
    no_window(&mut command);
    let result = command.output().context("lancement de rojo build")?;
    if !result.status.success() {
        let error = String::from_utf8_lossy(&result.stderr);
        anyhow::bail!("rojo build a échoué : {}", error.trim());
    }
    Ok(output)
}

/// Lance `rojo serve` pour le projet (sans fenêtre de console).
pub fn serve(project: &RobloxProject) -> Result<std::process::Child> {
    let rojo = find_rojo().context("Rojo est introuvable (installe-le avec Rokit : rokit add rojo-rbx/rojo)")?;
    let mut command = std::process::Command::new(rojo);
    command.arg("serve");
    if let Some(file) = &project.project_file {
        command.arg(file);
    }
    command.current_dir(&project.root).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    no_window(&mut command);
    command.spawn().context("lancement de rojo serve")
}

/// Ouvre un fichier avec son application (Roblox Studio pour une place).
pub fn open_file(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let mut command = std::process::Command::new("cmd");
        command.args(["/c", "start", ""]).arg(path);
        no_window(&mut command);
        command.spawn()?;
    }
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(path).spawn()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    std::process::Command::new("xdg-open").arg(path).spawn()?;
    Ok(())
}

fn no_window(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = command;
}

fn safe_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if cleaned.is_empty() { "place".into() } else { cleaned }
}

// — Skills livrés avec SupaCode

/// Skill embarqué : nom (dossier) et fichiers (chemin relatif, contenu).
pub struct BundledSkill {
    pub name: &'static str,
    pub summary: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

pub const SKILLS: &[BundledSkill] = &[
    BundledSkill {
        name: "roblox-build",
        summary: "Construire dans Studio : échelle, maquette, détails, matériaux, lumière, vérif par captures",
        files: &[
            ("SKILL.md", include_str!("../assets/skills/roblox-build/SKILL.md")),
            ("scripts/Builder.luau", include_str!("../assets/skills/roblox-build/scripts/Builder.luau")),
        ],
    },
    BundledSkill {
        name: "roblox-blender-model",
        summary: "Modèles 3D générés avec Blender en ligne de commande, exportés pour Roblox",
        files: &[
            ("SKILL.md", include_str!("../assets/skills/roblox-blender-model/SKILL.md")),
            ("scripts/model_template.py", include_str!("../assets/skills/roblox-blender-model/scripts/model_template.py")),
            ("scripts/preview.py", include_str!("../assets/skills/roblox-blender-model/scripts/preview.py")),
        ],
    },
    BundledSkill {
        name: "roblox-luau",
        summary: "Luau côté serveur et client : remotes sûrs, données, performances",
        files: &[("SKILL.md", include_str!("../assets/skills/roblox-luau/SKILL.md"))],
    },
    BundledSkill {
        name: "roblox-rojo",
        summary: "Rojo : structure du projet, serve, compilation de la place, Wally, Selene, StyLua",
        files: &[("SKILL.md", include_str!("../assets/skills/roblox-rojo/SKILL.md"))],
    },
];

/// Le skill est-il installé dans ce dossier de skills, et à jour ?
pub fn skill_status(skills_dir: &Path, skill: &BundledSkill) -> SkillStatus {
    let dir = skills_dir.join(skill.name);
    if !dir.join("SKILL.md").is_file() {
        return SkillStatus::Missing;
    }
    let current = skill.files.iter().all(|(path, content)| std::fs::read_to_string(dir.join(path)).is_ok_and(|on_disk| on_disk.replace("\r\n", "\n") == content.replace("\r\n", "\n")));
    if current { SkillStatus::Installed } else { SkillStatus::Outdated }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SkillStatus {
    Missing,
    Outdated,
    Installed,
}

/// Installe (ou met à jour) les skills Roblox dans un dossier de skills.
pub fn install_skills(skills_dir: &Path) -> Result<usize> {
    for skill in SKILLS {
        let dir = skills_dir.join(skill.name);
        for (path, content) in skill.files {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap_or(&dir))?;
            std::fs::write(&file, content).with_context(|| format!("écriture de {}", file.display()))?;
        }
    }
    Ok(SKILLS.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("supacode-roblox-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reconnait_un_projet_rojo() {
        let dir = temp_dir("rojo");
        std::fs::write(dir.join("default.project.json"), r#"{ "name": "BeeGarden", "servePort": 34999, "tree": { "$className": "DataModel" } }"#).unwrap();
        std::fs::write(dir.join("wally.toml"), "").unwrap();
        let project = detect(&dir).unwrap();
        assert_eq!(project.name, "BeeGarden");
        assert_eq!(project.serve_port, 34999);
        assert_eq!(project.toolchain, vec!["wally"]);
        assert!(project.project_file.is_some());
        assert!(open_in_studio(&project, &["BeeGarden - Roblox Studio".into()]));
        assert!(!open_in_studio(&project, &["Autre - Roblox Studio".into()]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ignore_un_dossier_ordinaire() {
        let dir = temp_dir("plain");
        std::fs::write(dir.join("README.md"), "").unwrap();
        assert!(detect(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installe_les_skills() {
        let dir = temp_dir("skills");
        assert_eq!(skill_status(&dir, &SKILLS[0]), SkillStatus::Missing);
        install_skills(&dir).unwrap();
        for skill in SKILLS {
            assert_eq!(skill_status(&dir, skill), SkillStatus::Installed, "{}", skill.name);
            let text = std::fs::read_to_string(dir.join(skill.name).join("SKILL.md")).unwrap().replace("\r\n", "\n");
            assert!(text.starts_with(&format!("---\nname: {}\ndescription: ", skill.name)), "{} : en-tête YAML incorrect", skill.name);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
