#!/usr/bin/env node
import { parseArgs } from "node:util";
import { loadConfig, Orchestrator, type AgentId } from "@supacode/core";
import { installHooks, uninstallHooks } from "./hooks.js";
import { formatDecision, printAgentEvent } from "./output.js";
import { listWorkflows, runWorkflowById, triggerWorkflows } from "./workflow.js";

const USAGE = `Utilisation :
  supacode route "<demande>"                 Affiche l'agent et le modèle choisis
  supacode run "<demande>" [options]         Lance la demande avec l'agent choisi
  supacode workflow list                     Liste les workflows de .supacode/workflows
  supacode workflow run <id> [options]       Lance un workflow
  supacode workflow trigger commit|push      Lance les workflows de ce déclencheur
  supacode hooks install|uninstall           Branche les workflows sur les hooks git

Options :
  --agent claude|codex   Force l'agent (run)
  --model <modèle>       Force le modèle (run)
  --json                 Affiche les événements bruts (une ligne JSON par événement)
  -y, --yes              Valide automatiquement les étapes « approve » (workflows)
  -h, --help             Affiche cette aide`;

const AGENTS: readonly AgentId[] = ["claude", "codex"];

function fail(message: string): never {
  console.error(`✖ ${message}\n\n${USAGE}`);
  process.exit(2);
}

async function main(): Promise<number> {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      agent: { type: "string" },
      model: { type: "string" },
      json: { type: "boolean", default: false },
      yes: { type: "boolean", short: "y", default: false },
      help: { type: "boolean", short: "h", default: false },
    },
  });

  const [command, ...rest] = positionals;
  if (values.help || !command) {
    console.log(USAGE);
    return 0;
  }

  const cwd = process.cwd();
  const orchestrator = new Orchestrator(await loadConfig(cwd));
  const signal = interruptSignal();

  switch (command) {
    case "route":
    case "run": {
      const prompt = rest.join(" ").trim();
      if (!prompt) fail("Il manque la demande.");
      if (values.agent !== undefined && !AGENTS.includes(values.agent as AgentId)) {
        fail(`Agent inconnu : « ${values.agent} » (attendu : ${AGENTS.join(" ou ")}).`);
      }
      const override = { agent: values.agent as AgentId | undefined, model: values.model };
      if (command === "route") {
        console.log(formatDecision(orchestrator.decide(prompt, override)));
        return 0;
      }
      return run(orchestrator, prompt, cwd, override, values.json, signal);
    }
    case "workflow": {
      const [action, arg] = rest;
      const options = { cwd, json: values.json, yes: values.yes, signal };
      if (action === "list") return listWorkflows(cwd);
      if (action === "run" && arg) return runWorkflowById(orchestrator, arg, options);
      if (action === "trigger" && (arg === "commit" || arg === "push")) return triggerWorkflows(orchestrator, arg, options);
      return fail("Usage : supacode workflow list | run <id> | trigger commit|push");
    }
    case "hooks": {
      const [action] = rest;
      if (action === "install") return installHooks(cwd);
      if (action === "uninstall") return uninstallHooks(cwd);
      return fail("Usage : supacode hooks install | uninstall");
    }
    default:
      return fail(`Commande inconnue : « ${command} ».`);
  }
}

/** Ctrl+C interrompt proprement l'agent ou le workflow en cours. */
function interruptSignal(): AbortSignal {
  const controller = new AbortController();
  process.once("SIGINT", () => controller.abort());
  return controller.signal;
}

async function run(
  orchestrator: Orchestrator,
  prompt: string,
  cwd: string,
  override: { agent?: AgentId; model?: string },
  json: boolean,
  signal: AbortSignal,
): Promise<number> {
  let exitCode = 1;
  for await (const event of orchestrator.run(prompt, { cwd, override, signal })) {
    if (event.type === "done") exitCode = event.exitCode;
    if (json) console.log(JSON.stringify(event));
    else printAgentEvent(event);
  }
  return exitCode;
}

main().then(
  (code) => process.exit(code),
  (err: unknown) => {
    console.error(`✖ ${err instanceof Error ? err.message : String(err)}`);
    process.exit(1);
  },
);
