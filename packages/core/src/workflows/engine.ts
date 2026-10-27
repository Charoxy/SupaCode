import { randomUUID } from "node:crypto";
import { spawnLines } from "../agents/process.js";
import type { Orchestrator } from "../orchestrator.js";
import type { RouteOverride } from "../router.js";
import type { AgentEvent } from "../types.js";
import { flattenSteps } from "./loader.js";
import type {
  AgentStep,
  StepKind,
  StepStatus,
  Workflow,
  WorkflowEvent,
  WorkflowRunOptions,
  WorkflowStep,
} from "./types.js";

/** Taille maximale conservée pour la sortie d'une étape (fin de sortie). */
const MAX_OUTPUT = 20_000;

interface StepResult {
  status: StepStatus;
  output: string;
}

/** État partagé d'une exécution. */
interface RunState {
  workflow: Workflow;
  orchestrator: Orchestrator;
  options: WorkflowRunOptions;
  results: Map<string, StepResult>;
  /** Étapes dont l'échec est rattrapé par une étape `if: "steps.<id>.failed"`. */
  handled: Set<string>;
  /** Groupes parallèles : leur échec est déjà compté via leurs étapes. */
  groups: Set<string>;
  git?: Promise<Record<string, string>>;
}

/** Exécute un workflow étape par étape et émet ses événements. */
export async function* runWorkflow(
  workflow: Workflow,
  orchestrator: Orchestrator,
  options: WorkflowRunOptions,
): AsyncGenerator<WorkflowEvent> {
  const runId = randomUUID().slice(0, 8);
  const all = flattenSteps(workflow.steps);
  const handled = new Set<string>();
  for (const id of all.map((s) => /^steps\.([\w-]+)\.failed$/.exec(s.if ?? "")?.[1])) {
    const target = all.find((s) => s.id === id);
    // Rattraper l'échec d'un groupe parallèle rattrape aussi celui de ses étapes.
    for (const step of target ? flattenSteps([target]) : []) handled.add(step.id);
  }
  const groups = new Set(all.filter((s) => "parallel" in s).map((s) => s.id));
  const state: RunState = { workflow, orchestrator, options, results: new Map(), handled, groups };

  yield { type: "workflow.start", runId, workflow: workflow.name };
  for (const step of workflow.steps) {
    if (options.signal?.aborted) break;
    yield* runStep(step, state);
  }

  const status = options.signal?.aborted ? "cancelled" : unhandledFailure(state) ? "failed" : "success";
  yield { type: "workflow.done", runId, status };
}

async function* runStep(step: WorkflowStep, state: RunState): AsyncGenerator<WorkflowEvent> {
  const kind = stepKind(step);
  yield { type: "step.start", stepId: step.id, kind, name: step.name ?? step.id };

  if (!shouldRun(step, state)) {
    state.results.set(step.id, { status: "skipped", output: "" });
    yield { type: "step.done", stepId: step.id, status: "skipped" };
    return;
  }

  let status: StepStatus = "failed";
  let exitCode: number | undefined;
  let output = "";
  try {
    if ("parallel" in step) {
      yield* merge(step.parallel.map((child) => runStep(child, state)));
      const failed = step.parallel.some((c) => state.results.get(c.id)?.status === "failed");
      status = failed ? "failed" : "success";
    } else if ("approve" in step) {
      const message = render(step.approve, await variables(step.approve, state));
      yield { type: "approval.request", stepId: step.id, message };
      const approved = (await state.options.approve?.(step.id, message)) ?? false;
      status = approved ? "success" : "failed";
      output = approved ? "validé" : "refusé";
    } else {
      const source = "run" in step ? runCommand(step.run, state) : runAgent(step, state);
      for await (const item of source) {
        if (item.type === "step.result") {
          exitCode = item.exitCode;
          output = item.output;
        } else {
          yield { type: "step.event", stepId: step.id, event: item };
        }
      }
      status = exitCode === 0 ? "success" : "failed";
    }
  } catch (err) {
    yield { type: "step.event", stepId: step.id, event: { type: "error", message: (err as Error).message } };
  }

  state.results.set(step.id, { status, output });
  yield exitCode === undefined ? { type: "step.done", stepId: step.id, status } : { type: "step.done", stepId: step.id, status, exitCode };
}

/** Événements d'une étape, puis son résultat final. */
type StepOutput = AgentEvent | { type: "step.result"; exitCode: number; output: string };

/** Commande shell ; le contexte est passé en variables d'environnement `SUPACODE_*`. */
async function* runCommand(command: string, state: RunState): AsyncGenerator<StepOutput> {
  let output = "";
  const env = await environment(state);
  for await (const item of spawnLines(command, [], { cwd: state.options.cwd, signal: state.options.signal, shell: true, env })) {
    if ("exitCode" in item) {
      yield { type: "step.result", exitCode: item.exitCode, output: output.trimEnd() };
      return;
    }
    output = tail(`${output}${item.line}\n`);
    yield { type: "log", message: item.line };
  }
}

/** Tâche d'agent ; les `{{variables}}` du prompt sont remplacées avant l'envoi. */
async function* runAgent(step: AgentStep, state: RunState): AsyncGenerator<StepOutput> {
  const prompt = render(step.prompt, await variables(step.prompt, state));
  const override: RouteOverride = {};
  if (step.agent && step.agent !== "auto") override.agent = step.agent;
  if (step.model) override.model = step.model;

  let output = "";
  let exitCode = 1;
  for await (const event of state.orchestrator.run(prompt, { cwd: state.options.cwd, override, signal: state.options.signal })) {
    if (event.type === "text") output = tail(`${output}${event.text}\n`);
    if (event.type === "done") {
      exitCode = event.exitCode;
      if (event.summary) output = tail(event.summary);
    }
    yield event;
  }
  yield { type: "step.result", exitCode, output: output.trimEnd() };
}

function shouldRun(step: WorkflowStep, state: RunState): boolean {
  if (step.if === "always") return true;
  if (step.if) {
    const [, id, expected] = step.if.split(".");
    return state.results.get(id ?? "")?.status === expected;
  }
  return !unhandledFailure(state);
}

function unhandledFailure(state: RunState): boolean {
  for (const [id, result] of state.results) {
    if (result.status === "failed" && !state.handled.has(id) && !state.groups.has(id)) return true;
  }
  return false;
}

function stepKind(step: WorkflowStep): StepKind {
  if ("parallel" in step) return "parallel";
  if ("approve" in step) return "approve";
  if ("run" in step) return "run";
  return "agent";
}

/**
 * Variables disponibles dans les prompts :
 * `{{git.diff}}` (diff indexé, sinon diff de travail), `{{git.branch}}`,
 * `{{steps.<id>.output}}`, `{{steps.<id>.status}}` et les clés de `options.context`.
 */
async function variables(template: string, state: RunState): Promise<Record<string, string>> {
  const vars: Record<string, string> = { ...state.options.context };
  for (const [id, result] of state.results) {
    vars[`steps.${id}.output`] = result.output;
    vars[`steps.${id}.status`] = result.status;
  }
  // Les commandes git ne sont lancées que si le modèle en a besoin.
  if (template.includes("{{git.")) Object.assign(vars, await gitVariables(state));
  return vars;
}

/** Contexte exposé aux commandes shell : SUPACODE_FILE, SUPACODE_STEP_LINT_OUTPUT… */
async function environment(state: RunState): Promise<Record<string, string>> {
  const env: Record<string, string> = {};
  const vars = await variables("", state);
  for (const [key, value] of Object.entries(vars)) {
    env[`SUPACODE_${key.replace(/^steps\./, "step.").replace(/[^\w]/g, "_").toUpperCase()}`] = value;
  }
  return env;
}

function gitVariables(state: RunState): Promise<Record<string, string>> {
  state.git ??= (async () => {
    const run = async (args: string[]) => {
      let out = "";
      try {
        for await (const item of spawnLines("git", args, { cwd: state.options.cwd })) {
          if ("line" in item && item.stream === "stdout") out += `${item.line}\n`;
        }
      } catch {
        // Pas de git ou pas de dépôt : variable vide.
      }
      return out.trimEnd();
    };
    const staged = await run(["diff", "--cached"]);
    return {
      "git.diff": staged || (await run(["diff"])),
      "git.branch": await run(["rev-parse", "--abbrev-ref", "HEAD"]),
    };
  })();
  return state.git;
}

/** Remplace les `{{nom}}` connus ; les inconnus sont laissés tels quels. */
export function render(template: string, vars: Record<string, string>): string {
  return template.replace(/\{\{\s*([\w.-]+)\s*\}\}/g, (match, key: string) => vars[key] ?? match);
}

function tail(text: string): string {
  return text.length > MAX_OUTPUT ? text.slice(-MAX_OUTPUT) : text;
}

/** Fusionne plusieurs flux d'événements dans l'ordre d'arrivée. */
async function* merge<T>(sources: AsyncGenerator<T>[]): AsyncGenerator<T> {
  const pending = new Map<number, Promise<{ index: number; result: IteratorResult<T> }>>();
  const next = (index: number) => sources[index]!.next().then((result) => ({ index, result }));
  sources.forEach((_, i) => pending.set(i, next(i)));
  while (pending.size > 0) {
    const { index, result } = await Promise.race(pending.values());
    if (result.done) {
      pending.delete(index);
    } else {
      pending.set(index, next(index));
      yield result.value;
    }
  }
}
