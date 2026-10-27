//! Modifications de structure d'un workflow, utilisées par l'éditeur visuel :
//! ajout, déplacement, suppression et renommage d'étapes, déclencheurs.

use std::collections::HashSet;

use crate::engine::{StepKind, flatten};
use crate::generate::slug;
use crate::workflows::{Step, Trigger, Workflow};

/// Étape de premier niveau, ou branche `j` du groupe parallèle `i`.
pub type StepPath = (usize, Option<usize>);

pub fn step_at(workflow: &Workflow, path: StepPath) -> Option<&Step> {
    let top = workflow.steps.get(path.0)?;
    match path.1 {
        None => Some(top),
        Some(j) => top.parallel.as_ref()?.get(j),
    }
}

pub fn step_at_mut(workflow: &mut Workflow, path: StepPath) -> Option<&mut Step> {
    let top = workflow.steps.get_mut(path.0)?;
    match path.1 {
        None => Some(top),
        Some(j) => top.parallel.as_mut()?.get_mut(j),
    }
}

fn ids(workflow: &Workflow) -> HashSet<String> {
    flatten(&workflow.steps).into_iter().map(|s| s.id.clone()).collect()
}

/// Id libre dérivé de `base` : « commande », « commande-2 »…
pub fn unique_id(workflow: &Workflow, base: &str) -> String {
    let ids = ids(workflow);
    let base = slug(base);
    if !ids.contains(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}-{n}")).find(|id| !ids.contains(id)).unwrap_or(base)
}

/// Ids des étapes terminées avant `path` : cibles possibles d'une condition.
/// Les branches d'un même groupe tournent en même temps : seules les étapes d'avant le groupe comptent.
pub fn previous_ids(workflow: &Workflow, path: StepPath) -> Vec<String> {
    workflow.steps[..path.0.min(workflow.steps.len())].iter().flat_map(|s| flatten(std::slice::from_ref(s))).map(|s| s.id.clone()).collect()
}

/// Ajoute un bloc après la sélection (ou dans le groupe parallèle sélectionné) et renvoie sa position.
pub fn add_step(workflow: &mut Workflow, selected: Option<StepPath>, kind: StepKind) -> StepPath {
    let (label, mut step) = match kind {
        StepKind::Run => ("Commande", Step { run: Some(String::new()), ..Default::default() }),
        StepKind::Agent => ("Agent", Step { prompt: Some(String::new()), agent: Some("auto".into()), ..Default::default() }),
        StepKind::Approve => ("Validation", Step { approve: Some("Valider ?".into()), ..Default::default() }),
        StepKind::Parallel => ("En parallèle", Step::default()),
    };
    step.id = unique_id(workflow, label);
    step.name = Some(label.to_owned());
    if kind == StepKind::Parallel {
        let a = unique_id(workflow, "branche");
        let b = format!("{a}-b");
        step.parallel = Some(vec![
            Step { id: a, name: Some("Branche 1".into()), run: Some(String::new()), ..Default::default() },
            Step { id: b, name: Some("Branche 2".into()), run: Some(String::new()), ..Default::default() },
        ]);
    }
    let selected = selected.filter(|p| step_at(workflow, *p).is_some());
    match selected {
        // Un groupe ne s'emboîte pas dans un autre groupe.
        Some((i, None)) if workflow.steps[i].parallel.is_some() && kind != StepKind::Parallel => {
            let children = workflow.steps[i].parallel.get_or_insert_with(Vec::new);
            children.push(step);
            (i, Some(children.len() - 1))
        }
        Some((i, Some(j))) if kind != StepKind::Parallel => {
            let children = workflow.steps[i].parallel.get_or_insert_with(Vec::new);
            children.insert(j + 1, step);
            (i, Some(j + 1))
        }
        Some((i, _)) => {
            workflow.steps.insert(i + 1, step);
            (i + 1, None)
        }
        None => {
            workflow.steps.push(step);
            (workflow.steps.len() - 1, None)
        }
    }
}

/// Déplace l'étape d'un cran (dans sa liste) ; renvoie sa nouvelle position.
pub fn move_step(workflow: &mut Workflow, path: StepPath, delta: isize) -> Option<StepPath> {
    let (i, sub) = path;
    let list = match sub {
        None => &mut workflow.steps,
        Some(_) => workflow.steps.get_mut(i)?.parallel.as_mut()?,
    };
    let from = sub.unwrap_or(i);
    let to = from.checked_add_signed(delta).filter(|t| *t < list.len() && from < list.len())?;
    list.swap(from, to);
    Some(match sub {
        None => (to, None),
        Some(_) => (i, Some(to)),
    })
}

pub fn remove_step(workflow: &mut Workflow, path: StepPath) {
    match path {
        (i, None) if i < workflow.steps.len() => {
            workflow.steps.remove(i);
        }
        (i, Some(j)) => {
            if let Some(children) = workflow.steps.get_mut(i).and_then(|s| s.parallel.as_mut()).filter(|c| j < c.len()) {
                children.remove(j);
            }
        }
        _ => {}
    }
}

/// Renomme l'étape et met à jour les conditions et `{{steps.<id>…}}` qui la citent.
/// Refuse (faux) un id vide, invalide ou déjà pris.
pub fn rename_step(workflow: &mut Workflow, path: StepPath, new: &str) -> bool {
    let Some(old) = step_at(workflow, path).map(|s| s.id.clone()) else { return false };
    let valid = !new.is_empty() && new.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if new == old || !valid || ids(workflow).contains(new) {
        return false;
    }
    fn replace(steps: &mut [Step], from: &str, to: &str) {
        for step in steps {
            for field in [&mut step.condition, &mut step.prompt, &mut step.approve] {
                if let Some(text) = field {
                    *text = text.replace(from, to);
                }
            }
            if let Some(children) = &mut step.parallel {
                replace(children, from, to);
            }
        }
    }
    replace(&mut workflow.steps, &format!("steps.{old}."), &format!("steps.{new}."));
    if let Some(step) = step_at_mut(workflow, path) {
        step.id = new.to_owned();
    }
    true
}

/// Active ou retire un déclencheur (« manual », « commit », « push » ou « save » avec son motif).
pub fn toggle_trigger(workflow: &mut Workflow, name: &str, glob: &str) {
    let mut triggers = workflow.triggers();
    let position = triggers.iter().position(|t| match t {
        Trigger::Save { .. } => name == "save",
        Trigger::Named(n) => n == name,
    });
    match position {
        Some(index) => {
            triggers.remove(index);
        }
        None if name == "save" => triggers.push(Trigger::Save { save: glob.to_owned() }),
        None => triggers.push(Trigger::Named(name.to_owned())),
    }
    // Sans déclencheur (ou seulement « manual », la valeur par défaut), le champ est omis.
    workflow.on = if triggers.is_empty() || triggers == [Trigger::Named("manual".into())] { None } else { Some(triggers) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::validate;

    fn workflow() -> Workflow {
        serde_json::from_str(
            r#"{"name":"t","steps":[
                {"id":"lint","run":"npm run lint"},
                {"id":"g","parallel":[{"id":"tests","run":"npm test"}]},
                {"id":"fix","if":"steps.lint.failed","prompt":"Corrige {{steps.lint.output}}"}
            ]}"#,
        )
        .unwrap()
    }

    #[test]
    fn ajoute_au_bon_endroit() {
        let mut wf = workflow();
        assert_eq!(add_step(&mut wf, Some((0, None)), StepKind::Run), (1, None));
        assert_eq!(wf.steps[1].id, "commande");
        // Groupe sélectionné : le bloc devient une branche ; un groupe, lui, va après.
        assert_eq!(add_step(&mut wf, Some((2, None)), StepKind::Approve), (2, Some(1)));
        assert_eq!(add_step(&mut wf, Some((2, Some(0))), StepKind::Parallel), (3, None));
        assert_eq!(add_step(&mut wf, None, StepKind::Run), (wf.steps.len() - 1, None));
        assert_eq!(wf.steps.last().unwrap().id, "commande-2");
        // Les blocs ajoutés sont à compléter : la validation le signale.
        assert!(validate(&wf).is_err());
    }

    #[test]
    fn deplace_et_supprime() {
        let mut wf = workflow();
        assert_eq!(move_step(&mut wf, (0, None), 1), Some((1, None)));
        assert_eq!(wf.steps[0].id, "g");
        assert_eq!(move_step(&mut wf, (0, None), -1), None);
        remove_step(&mut wf, (0, Some(0)));
        assert!(wf.steps[0].parallel.as_ref().unwrap().is_empty());
        remove_step(&mut wf, (0, None));
        assert_eq!(wf.steps.len(), 2);
    }

    #[test]
    fn renomme_avec_les_references() {
        let mut wf = workflow();
        assert!(rename_step(&mut wf, (0, None), "verif"));
        assert_eq!(wf.steps[2].condition.as_deref(), Some("steps.verif.failed"));
        assert_eq!(wf.steps[2].prompt.as_deref(), Some("Corrige {{steps.verif.output}}"));
        assert!(!rename_step(&mut wf, (0, None), "tests"), "id déjà pris");
        assert!(!rename_step(&mut wf, (0, None), "pas valide"));
    }

    #[test]
    fn declencheurs() {
        let mut wf = workflow();
        toggle_trigger(&mut wf, "commit", "");
        assert_eq!(wf.on, Some(vec![Trigger::Named("manual".into()), Trigger::Named("commit".into())]));
        toggle_trigger(&mut wf, "save", "src/**/*.ts");
        toggle_trigger(&mut wf, "commit", "");
        toggle_trigger(&mut wf, "save", "");
        assert_eq!(wf.on, None);
        assert_eq!(previous_ids(&wf, (1, Some(0))), ["lint"]);
        assert_eq!(previous_ids(&wf, (2, None)), ["lint", "g", "tests"]);
    }
}
