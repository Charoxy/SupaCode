#!/usr/bin/env node
import { parseArgs } from "node:util";
import { loadConfig, Orchestrator, type AgentEvent, type AgentId, type RouteDecision } from "@supacode/core";

const USAGE = `Utilisation :
  supacode route "<demande>"                 Affiche l'agent et le modèle choisis
  supacode run "<demande>" [options]         Lance la demande avec l'agent choisi

Options de run :
  --agent claude|codex   Force l'agent
  --model <modèle>       Force le modèle
  --json                 Affiche les événements bruts (une ligne JSON par événement)
  -h, --help             Affiche cette aide`;

const AGENTS: readonly AgentId[] = ["claude", "codex"];

function formatDecision(d: RouteDecision): string {
  return `${d.agent}${d.model ? ` (${d.model})` : ""} : ${d.reason}`;
}

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
      help: { type: "boolean", short: "h", default: false },
    },
  });

  const [command, ...rest] = positionals;
  if (values.help || !command) {
    console.log(USAGE);
    return 0;
  }

  const prompt = rest.join(" ").trim();
  if (!prompt) fail("Il manque la demande.");
  if (values.agent !== undefined && !AGENTS.includes(values.agent as AgentId)) {
    fail(`Agent inconnu : « ${values.agent} » (attendu : ${AGENTS.join(" ou ")}).`);
  }
  const override = { agent: values.agent as AgentId | undefined, model: values.model };

  const cwd = process.cwd();
  const orchestrator = new Orchestrator(await loadConfig(cwd));

  switch (command) {
    case "route":
      console.log(formatDecision(orchestrator.decide(prompt, override)));
      return 0;
    case "run":
      return run(orchestrator, prompt, cwd, override, values.json);
    default:
      fail(`Commande inconnue : « ${command} ».`);
  }
}

async function run(
  orchestrator: Orchestrator,
  prompt: string,
  cwd: string,
  override: { agent?: AgentId; model?: string },
  json: boolean,
): Promise<number> {
  const controller = new AbortController();
  process.once("SIGINT", () => controller.abort());

  let exitCode = 1;
  for await (const event of orchestrator.run(prompt, { cwd, override, signal: controller.signal })) {
    if (event.type === "done") exitCode = event.exitCode;
    if (json) console.log(JSON.stringify(event));
    else print(event);
  }
  return exitCode;
}

function print(event: AgentEvent): void {
  switch (event.type) {
    case "route":
      console.log(`▶ ${formatDecision(event.decision)}`);
      break;
    case "text":
      console.log(event.text);
      break;
    case "tool":
      console.log(`· ${event.name}${event.detail ? ` ${event.detail}` : ""}`);
      break;
    case "error":
      console.error(`✖ ${event.message}`);
      break;
    // Les logs et la fin ne sont visibles qu'en --json.
  }
}

main().then(
  (code) => process.exit(code),
  (err: unknown) => {
    console.error(`✖ ${err instanceof Error ? err.message : String(err)}`);
    process.exit(1);
  },
);
