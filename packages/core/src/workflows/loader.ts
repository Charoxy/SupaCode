import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import type { Workflow, WorkflowFile, WorkflowStep, WorkflowTrigger } from "./types.js";

export const WORKFLOWS_DIR = join(".supacode", "workflows");

const CONDITION = /^(always|steps\.[\w-]+\.(success|failed|skipped))$/;
const STEP_KEYS = ["run", "prompt", "approve", "parallel"] as const;

/** Valide un objet JSON et le convertit en `Workflow` ; lève une erreur lisible sinon. */
export function parseWorkflow(value: unknown, source = "workflow"): Workflow {
  const fail = (where: string, message: string): never => {
    throw new Error(`${source}${where ? ` (${where})` : ""} : ${message}`);
  };

  if (!isObject(value)) fail("", "le workflow doit être un objet JSON");
  const wf = value as Record<string, unknown>;
  if (typeof wf.name !== "string" || !wf.name.trim()) fail("name", "nom manquant");
  if (wf.description !== undefined && typeof wf.description !== "string") fail("description", "doit être une chaîne");

  if (wf.on !== undefined) {
    if (!Array.isArray(wf.on)) fail("on", "doit être une liste de déclencheurs");
    (wf.on as unknown[]).forEach((trigger, i) => {
      if (!isTrigger(trigger)) fail(`on[${i}]`, `déclencheur inconnu ${JSON.stringify(trigger)} (manual, commit, push ou { "save": "<motif>" })`);
    });
  }

  if (!Array.isArray(wf.steps) || wf.steps.length === 0) fail("steps", "au moins une étape est requise");
  const ids = new Set<string>();
  const checkSteps = (steps: unknown[], path: string) => {
    steps.forEach((raw, i) => {
      const where = `${path}[${i}]`;
      if (!isObject(raw)) fail(where, "une étape doit être un objet");
      const step = raw as Record<string, unknown>;
      if (typeof step.id !== "string" || !/^[\w-]+$/.test(step.id)) fail(where, "id manquant ou invalide (lettres, chiffres, - et _)");
      const id = step.id as string;
      if (ids.has(id)) fail(where, `id « ${id} » en double`);
      ids.add(id);

      const kinds = STEP_KEYS.filter((k) => step[k] !== undefined);
      if (kinds.length !== 1) fail(where, `une étape doit avoir exactement un de : ${STEP_KEYS.join(", ")}`);
      const kind = kinds[0];
      if (kind === "parallel") {
        if (!Array.isArray(step.parallel) || step.parallel.length === 0) fail(`${where}.parallel`, "liste d'étapes non vide attendue");
        checkSteps(step.parallel as unknown[], `${where}.parallel`);
      } else if (typeof step[kind as string] !== "string" || !(step[kind as string] as string).trim()) {
        fail(`${where}.${kind}`, "chaîne non vide attendue");
      }

      if (step.agent !== undefined && !["auto", "claude", "codex"].includes(step.agent as string)) {
        fail(`${where}.agent`, "attendu : auto, claude ou codex");
      }
      if (step.model !== undefined && typeof step.model !== "string") fail(`${where}.model`, "doit être une chaîne");
      if (step.name !== undefined && typeof step.name !== "string") fail(`${where}.name`, "doit être une chaîne");
      if (step.if !== undefined && (typeof step.if !== "string" || !CONDITION.test(step.if))) {
        fail(`${where}.if`, `condition invalide ${JSON.stringify(step.if)} (always ou steps.<id>.success|failed|skipped)`);
      }
    });
  };
  checkSteps(wf.steps as unknown[], "steps");

  for (const step of flattenSteps((wf as unknown as Workflow).steps)) {
    const ref = step.if?.split(".")[1];
    if (step.if !== "always" && ref !== undefined && !ids.has(ref)) fail(`étape ${step.id}`, `la condition cite une étape inconnue « ${ref} »`);
  }

  return value as Workflow;
}

/** Charge tous les `.supacode/workflows/*.json` du projet (triés par id). */
export async function loadWorkflows(projectDir: string): Promise<WorkflowFile[]> {
  const dir = join(projectDir, WORKFLOWS_DIR);
  let names: string[];
  try {
    names = await readdir(dir);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw err;
  }

  const files: WorkflowFile[] = [];
  for (const name of names.filter((n) => n.endsWith(".json")).sort()) {
    const path = join(dir, name);
    let json: unknown;
    try {
      json = JSON.parse(await readFile(path, "utf8"));
    } catch (err) {
      throw new Error(`${name} : JSON invalide (${(err as Error).message})`);
    }
    files.push({ id: name.slice(0, -".json".length), path, workflow: parseWorkflow(json, name) });
  }
  return files;
}

/** Workflows qui ont le déclencheur donné (sans déclencheur : manuel seulement). */
export function workflowsFor(files: WorkflowFile[], trigger: "manual" | "commit" | "push"): WorkflowFile[] {
  return files.filter((f) => (f.workflow.on ?? ["manual"]).includes(trigger));
}

function isTrigger(value: unknown): value is WorkflowTrigger {
  if (value === "manual" || value === "commit" || value === "push") return true;
  return isObject(value) && typeof value.save === "string" && value.save.length > 0;
}

/** Liste plate des étapes, `parallel` compris. */
export function flattenSteps(steps: WorkflowStep[]): WorkflowStep[] {
  return steps.flatMap((s) => ("parallel" in s ? [s, ...flattenSteps(s.parallel)] : [s]));
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
