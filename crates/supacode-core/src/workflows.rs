//! Format des workflows (`.supacode/workflows/<id>.json`), identique à celui de la CLI TypeScript.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Trigger {
    /// "manual", "commit" ou "push".
    Named(String),
    /// À l'enregistrement d'un fichier correspondant au motif.
    Save { save: String },
}

impl Trigger {
    /// Libellé court pour les étiquettes : « commit », « save src/**/*.ts ».
    pub fn label(&self) -> String {
        match self {
            Trigger::Named(name) if name == "manual" => "manuel".into(),
            Trigger::Named(name) => name.clone(),
            Trigger::Save { save } => format!("save {save}"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, rename = "if", skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approve: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel: Option<Vec<Step>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<Vec<Trigger>>,
    pub steps: Vec<Step>,
}

impl Workflow {
    /// Déclencheurs, `manual` par défaut.
    pub fn triggers(&self) -> Vec<Trigger> {
        self.on.clone().unwrap_or_else(|| vec![Trigger::Named("manual".into())])
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowFile {
    /// Nom du fichier sans extension.
    pub id: String,
    pub workflow: Workflow,
}

/// Charge les workflows du projet, triés par id ; dossier absent → liste vide.
pub fn load_workflows(project: &Path) -> Result<Vec<WorkflowFile>> {
    let dir = project.join(".supacode").join("workflows");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut files: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let text = fs::read_to_string(&path).with_context(|| format!("{name} : lecture impossible"))?;
            let workflow: Workflow = serde_json::from_str(&text).with_context(|| format!("{name} : JSON invalide"))?;
            if workflow.steps.is_empty() {
                bail!("{name} : au moins une étape est requise");
            }
            Ok(WorkflowFile { id: name.trim_end_matches(".json").to_owned(), workflow })
        })
        .collect()
}

/// Enregistre un nouveau workflow sous `.supacode/workflows/<id>.json` sans écraser l'existant
/// (`<id>-2`, `<id>-3`… si besoin) ; renvoie l'id retenu.
pub fn save_workflow(project: &Path, id: &str, workflow: &Workflow) -> Result<String> {
    let dir = project.join(".supacode").join("workflows");
    fs::create_dir_all(&dir).with_context(|| format!("impossible de créer {}", dir.display()))?;
    let mut candidate = id.to_owned();
    let mut n = 2;
    while dir.join(format!("{candidate}.json")).exists() {
        candidate = format!("{id}-{n}");
        n += 1;
    }
    let text = serde_json::to_string_pretty(workflow)? + "
";
    fs::write(dir.join(format!("{candidate}.json")), text).context("écriture du workflow impossible")?;
    Ok(candidate)
}

/// Réécrit `.supacode/workflows/<id>.json` (modification d'un workflow existant).
pub fn write_workflow(project: &Path, id: &str, workflow: &Workflow) -> Result<()> {
    let dir = project.join(".supacode").join("workflows");
    fs::create_dir_all(&dir).with_context(|| format!("impossible de créer {}", dir.display()))?;
    let text = serde_json::to_string_pretty(workflow)? + "
";
    fs::write(dir.join(format!("{id}.json")), text).context("écriture du workflow impossible")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charge_et_libelle_les_declencheurs() {
        let dir = std::env::temp_dir().join(format!("supacode-wf-{}", std::process::id()));
        let wf_dir = dir.join(".supacode").join("workflows");
        fs::create_dir_all(&wf_dir).unwrap();
        fs::write(
            wf_dir.join("verifs.json"),
            r#"{"name":"Vérifs","on":["commit",{"save":"src/**/*.ts"}],"steps":[{"id":"lint","run":"npm run lint"}]}"#,
        )
        .unwrap();
        let files = load_workflows(&dir).unwrap();
        assert_eq!(files[0].id, "verifs");
        let labels: Vec<_> = files[0].workflow.triggers().iter().map(Trigger::label).collect();
        assert_eq!(labels, ["commit", "save src/**/*.ts"]);
        assert_eq!(Trigger::Named("manual".into()).label(), "manuel");
        assert!(load_workflows(&dir.join("absent")).unwrap().is_empty());
        // Un nouveau workflow du même nom ne remplace pas l'existant.
        let id = save_workflow(&dir, "verifs", &files[0].workflow).unwrap();
        assert_eq!(id, "verifs-2");
        assert_eq!(load_workflows(&dir).unwrap().len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
