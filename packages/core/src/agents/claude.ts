import type { Agent, AgentEvent, AgentRequest } from "../types.js";
import { commandExists, streamAgent } from "./process.js";

/** Pilote la CLI Claude Code en mode non interactif (`claude -p`). */
export class ClaudeAgent implements Agent {
  readonly id = "claude" as const;

  constructor(private readonly command = "claude") {}

  isAvailable(): Promise<boolean> {
    return commandExists(this.command);
  }

  run(request: AgentRequest, signal?: AbortSignal): AsyncIterable<AgentEvent> {
    const args = ["-p", request.prompt, "--output-format", "stream-json", "--verbose", "--permission-mode", "acceptEdits"];
    if (request.model) args.push("--model", request.model);
    return streamAgent(this.command, args, { cwd: request.cwd, signal }, parseClaudeLine);
  }
}

/** Convertit une ligne `stream-json` de Claude Code en événements SupaCode. */
export function parseClaudeLine(line: string): AgentEvent[] {
  const msg = parseJsonObject(line);
  if (!msg) return [{ type: "log", message: line }];

  if (msg.type === "assistant") {
    const content = (msg.message as { content?: unknown } | undefined)?.content;
    if (!Array.isArray(content)) return [];
    const events: AgentEvent[] = [];
    for (const block of content as Array<Record<string, unknown>>) {
      if (block.type === "text" && typeof block.text === "string" && block.text.trim()) {
        events.push({ type: "text", text: block.text });
      } else if (block.type === "tool_use" && typeof block.name === "string") {
        const detail = toolDetail(block.input);
        events.push(detail ? { type: "tool", name: block.name, detail } : { type: "tool", name: block.name });
      }
    }
    return events;
  }

  if (msg.type === "result") {
    const result = typeof msg.result === "string" ? msg.result : undefined;
    if (msg.is_error === true || (typeof msg.subtype === "string" && msg.subtype !== "success")) {
      return [{ type: "error", message: result ?? `Claude a échoué (${String(msg.subtype)})` }];
    }
    return [result !== undefined ? { type: "done", exitCode: 0, summary: result } : { type: "done", exitCode: 0 }];
  }

  // init, résultats d'outils… : rien à afficher.
  return [];
}

/** Résumé court de l'entrée d'un outil (fichier, commande, motif…). */
function toolDetail(input: unknown): string | undefined {
  if (typeof input !== "object" || input === null) return undefined;
  const fields = input as Record<string, unknown>;
  for (const key of ["file_path", "path", "command", "pattern", "url", "description"]) {
    const value = fields[key];
    if (typeof value === "string" && value) return value;
  }
  return undefined;
}

/** Parse une ligne JSON ; `undefined` si ce n'est pas un objet JSON. */
export function parseJsonObject(line: string): Record<string, unknown> | undefined {
  try {
    const value: unknown = JSON.parse(line);
    return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
  } catch {
    return undefined;
  }
}
