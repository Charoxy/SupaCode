import type { Agent, AgentEvent, AgentRequest } from "../types.js";
import { parseJsonObject } from "./claude.js";
import { commandExists, streamAgent } from "./process.js";

/** Pilote la CLI Codex en mode non interactif (`codex exec`). */
export class CodexAgent implements Agent {
  readonly id = "codex" as const;

  constructor(private readonly command = "codex") {}

  isAvailable(): Promise<boolean> {
    return commandExists(this.command);
  }

  run(request: AgentRequest, signal?: AbortSignal): AsyncIterable<AgentEvent> {
    // `--sandbox workspace-write` : équivalent de l'ancien `--full-auto` (retiré des versions récentes).
    const args = ["exec", "--json", "--sandbox", "workspace-write"];
    if (request.model) args.push("--model", request.model);
    args.push(request.prompt);
    return streamAgent(this.command, args, { cwd: request.cwd, signal }, parseCodexLine);
  }
}

/** Convertit une ligne JSONL de `codex exec --json` en événements SupaCode. */
export function parseCodexLine(line: string): AgentEvent[] {
  const msg = parseJsonObject(line);
  if (!msg) return [{ type: "log", message: line }];

  if (msg.type === "item.completed" && typeof msg.item === "object" && msg.item !== null) {
    const item = msg.item as Record<string, unknown>;
    switch (item.type) {
      case "agent_message":
        if (typeof item.text === "string") return [{ type: "text", text: item.text }];
        break;
      case "command_execution":
        return [typeof item.command === "string" ? { type: "tool", name: "shell", detail: item.command } : { type: "tool", name: "shell" }];
      case "file_change": {
        const paths = Array.isArray(item.changes)
          ? (item.changes as Array<{ path?: unknown }>).map((c) => c.path).filter((p): p is string => typeof p === "string")
          : [];
        return [paths.length ? { type: "tool", name: "edit", detail: paths.join(", ") } : { type: "tool", name: "edit" }];
      }
    }
  }

  if (msg.type === "error" || msg.type === "turn.failed") {
    const nested = msg.error as { message?: unknown } | undefined;
    const message =
      typeof msg.message === "string" ? msg.message : typeof nested?.message === "string" ? nested.message : "Codex a échoué";
    // Codex relaie parfois l'erreur brute de l'API sous forme de JSON : on en extrait le message.
    const apiError = parseJsonObject(message)?.error as { message?: unknown } | undefined;
    return [{ type: "error", message: typeof apiError?.message === "string" ? apiError.message : message }];
  }

  return [{ type: "log", message: line }];
}
