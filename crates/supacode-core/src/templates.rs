//! Workflows prêts à l'emploi, adaptés au projet.

use std::path::Path;

use crate::workflows::{Step, Workflow};

/// Commande de test du projet, devinée d'après ses fichiers.
pub fn test_command(root: &Path) -> String {
    let has = |name: &str| root.join(name).exists();
    if has("Cargo.toml") {
        return "cargo test".into();
    }
    if has("package.json") {
        let manager = if has("pnpm-lock.yaml") {
            "pnpm"
        } else if has("yarn.lock") {
            "yarn"
        } else if has("bun.lockb") || has("bun.lock") {
            "bun"
        } else {
            "npm"
        };
        return format!("{manager} test");
    }
    if has("pyproject.toml") || has("pytest.ini") || has("setup.cfg") || has("tox.ini") {
        return "python -m pytest".into();
    }
    if has("go.mod") {
        return "go test ./...".into();
    }
    if has("pom.xml") {
        return "mvn test".into();
    }
    if has("build.gradle") || has("build.gradle.kts") {
        return "gradle test".into();
    }
    if root.read_dir().into_iter().flatten().flatten().any(|e| e.path().extension().is_some_and(|x| x == "sln" || x == "csproj")) {
        return "dotnet test".into();
    }
    "npm test".into()
}

/// « Correctif : tests d'abord » : un test reproduit le bug et doit échouer avant toute
/// correction, puis passer après. Sans test rouge, pas de correctif.
pub fn test_first(root: &Path) -> Workflow {
    let tests = test_command(root);
    let step = |id: &str, name: &str| Step { id: id.into(), name: Some(name.into()), ..Default::default() };
    Workflow {
        name: "Correctif : tests d'abord".into(),
        description: Some("Un test reproduit le bug avant toute correction : il doit échouer, puis passer. Un correctif sans test rouge n'est qu'un déménagement du bug.".into()),
        on: None,
        ask: Some("Décris le bug à corriger".into()),
        steps: vec![
            Step {
                prompt: Some(
                    "Bug signalé : {{saisie}}\n\nÉcris UNIQUEMENT un test automatisé qui reproduit ce bug, dans les conventions de test du projet. \
                     Ne corrige pas le code : ce test doit échouer tant que le bug existe. Termine par une phrase qui dit où est le test."
                        .into(),
                ),
                agent: Some("auto".into()),
                ..step("test", "Écrire le test qui reproduit le bug")
            },
            Step { run: Some(tests.clone()), expect: Some("fail".into()), ..step("rouge", "Le test doit échouer") },
            Step {
                prompt: Some(
                    "Bug signalé : {{saisie}}\n\nUn test qui reproduit ce bug vient d'être écrit ({{steps.test.output}}). Il échoue :\n{{steps.rouge.output}}\n\n\
                     Corrige le code pour que ce test passe, sans modifier le test et sans casser les autres. Explique la cause du bug en deux phrases."
                        .into(),
                ),
                agent: Some("auto".into()),
                ..step("correctif", "Corriger le code")
            },
            Step { run: Some(tests), ..step("vert", "Tous les tests passent") },
            Step {
                approve: Some("Le test échouait avant le correctif et passe maintenant. Relis le diff : saurais-tu expliquer ce changement dans six mois ?".into()),
                ..step("relecture", "Relecture")
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::{test_command, test_first};
    use crate::generate::validate;

    #[test]
    fn modele_valide() {
        let root = std::env::temp_dir().join(format!("supacode-modele-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        assert_eq!(test_command(&root), "cargo test");
        let workflow = test_first(&root);
        validate(&workflow).unwrap();
        assert_eq!(workflow.steps[1].expect.as_deref(), Some("fail"));
        let _ = std::fs::remove_dir_all(root);
    }
}
