import { createInterface } from "node:readline/promises";
import { loadWorkflows, runWorkflow, workflowsFor, type Orchestrator, type Workflow, type WorkflowEvent } from "@supacode/core";
import { printAgentEvent } from "./output.js";

export interface WorkflowCliOptions {
  cwd: string;
  json: boolean;
  yes: boolean;
  signal: AbortSignal;
}

const STATUS_ICONS = { success: "✔", failed: "✖", skipped: "↷", cancelled: "■" } as const;

export async function listWorkflows(cwd: string): Promise<number> {
  const files = await loadWorkflows(cwd);
  if (files.length === 0) {
    console.log("Aucun workflow dans .supacode/workflows.");
    return 0;
  }
  for (const { id, workflow } of files) {
    const triggers = (workflow.on ?? ["manual"]).map((t) => (typeof t === "string" ? t : `save ${t.save}`)).join(", ");
    console.log(`${id}  ${workflow.name}  [${triggers}]${workflow.description ? `\n    ${workflow.description}` : ""}`);
  }
  return 0;
}

export async function runWorkflowById(orchestrator: Orchestrator, id: string, options: WorkflowCliOptions): Promise<number> {
  const file = (await loadWorkflows(options.cwd)).find((f) => f.id === id);
  if (!file) {
    console.error(`✖ Workflow « ${id} » introuvable (voir supacode workflow list).`);
    return 1;
  }
  return execute(orchestrator, file.workflow, options);
}

/** Appelé par les hooks git : lance chaque workflow du déclencheur, s'arrête au premier échec. */
export async function triggerWorkflows(orchestrator: Orchestrator, trigger: "commit" | "push", options: WorkflowCliOptions): Promise<number> {
  for (const file of workflowsFor(await loadWorkflows(options.cwd), trigger)) {
    const code = await execute(orchestrator, file.workflow, options);
    if (code !== 0) return code;
  }
  return 0;
}

async function execute(orchestrator: Orchestrator, workflow: Workflow, options: WorkflowCliOptions): Promise<number> {
  let code = 1;
  const runOptions = { cwd: options.cwd, signal: options.signal, approve: (_: string, message: string) => approve(message, options.yes) };
  for await (const event of runWorkflow(workflow, orchestrator, runOptions)) {
    if (event.type === "workflow.done") code = event.status === "success" ? 0 : 1;
    if (options.json) console.log(JSON.stringify(event));
    else printWorkflowEvent(event);
  }
  return code;
}

function printWorkflowEvent(event: WorkflowEvent): void {
  switch (event.type) {
    case "workflow.start":
      console.log(`▶ Workflow « ${event.workflow} »`);
      break;
    case "step.start":
      console.log(`\n● ${event.name} (${event.kind})`);
      break;
    case "step.event":
      if (event.event.type === "log") console.log(`  │ ${event.event.message}`);
      else printAgentEvent(event.event, "  │ ");
      break;
    case "approval.request":
      console.log(`  ? ${event.message}`);
      break;
    case "step.done":
      console.log(`  ${STATUS_ICONS[event.status]} ${event.stepId} : ${event.status}${event.exitCode ? ` (code ${event.exitCode})` : ""}`);
      break;
    case "workflow.done":
      console.log(`\n${STATUS_ICONS[event.status]} Workflow terminé : ${event.status}`);
      break;
  }
}

/** Validation dans le terminal ; sans terminal (ex. hook git) et sans --yes, c'est un refus. */
async function approve(message: string, yes: boolean): Promise<boolean> {
  if (yes) return true;
  if (!process.stdin.isTTY) {
    console.log("  (pas de terminal interactif : refus, utilise --yes pour valider automatiquement)");
    return false;
  }
  const rl = createInterface({ input: process.stdin, output: process.stdout });
  try {
    return /^(o|oui|y|yes)$/i.test((await rl.question(`  ${message} [o/N] `)).trim());
  } finally {
    rl.close();
  }
}
