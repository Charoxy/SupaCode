import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import type { AgentEvent } from "../types.js";

export type SpawnLine = { stream: "stdout" | "stderr"; line: string };
export type SpawnItem = SpawnLine | { exitCode: number };

export interface SpawnOptions {
  cwd: string;
  signal?: AbortSignal;
}

/**
 * Lance une commande et renvoie ses lignes (stdout et stderr) au fil de l'eau,
 * puis `{ exitCode }` une fois le processus terminé.
 * Lève une erreur si la commande ne peut pas être lancée (ex. introuvable).
 */
export async function* spawnLines(cmd: string, args: string[], options: SpawnOptions): AsyncGenerator<SpawnItem> {
  const child = spawn(cmd, args, {
    cwd: options.cwd,
    signal: options.signal,
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });

  const queue: SpawnItem[] = [];
  let wake: (() => void) | undefined;
  let closed = false;
  let failure: Error | undefined;
  const notify = () => {
    wake?.();
    wake = undefined;
  };

  createInterface({ input: child.stdout }).on("line", (line) => {
    queue.push({ stream: "stdout", line });
    notify();
  });
  createInterface({ input: child.stderr }).on("line", (line) => {
    queue.push({ stream: "stderr", line });
    notify();
  });
  child.on("error", (err) => {
    // Une interruption volontaire n'est pas une erreur : le code de sortie suffit.
    if (err.name !== "AbortError") {
      failure = err;
      notify();
    }
  });
  child.on("close", (code, signal) => {
    queue.push({ exitCode: code ?? (signal ? 130 : 1) });
    closed = true;
    notify();
  });

  while (true) {
    const item = queue.shift();
    if (item) {
      if ("exitCode" in item && failure) throw failure;
      yield item;
      if ("exitCode" in item) return;
      continue;
    }
    if (failure) throw failure;
    if (closed) return;
    await new Promise<void>((resolve) => (wake = resolve));
  }
}

/** Vrai si `cmd --version` s'exécute avec succès. */
export function commandExists(cmd: string): Promise<boolean> {
  return new Promise((resolve) => {
    const child = spawn(cmd, ["--version"], { stdio: "ignore", windowsHide: true, timeout: 15_000 });
    child.on("error", () => resolve(false));
    child.on("close", (code) => resolve(code === 0));
  });
}

/**
 * Lance la CLI d'un agent et convertit sa sortie en événements :
 * stdout passe par `parse`, stderr devient des `log`. Se termine toujours par `done`.
 */
export async function* streamAgent(
  cmd: string,
  args: string[],
  options: SpawnOptions,
  parse: (line: string) => AgentEvent[],
): AsyncGenerator<AgentEvent> {
  let summary: string | undefined;
  let lastText: string | undefined;
  let failed = false;
  let lastError: string | undefined;
  const stderrTail: string[] = [];
  try {
    for await (const item of spawnLines(cmd, args, options)) {
      if ("exitCode" in item) {
        // Un échec signalé par l'agent ne doit pas finir en code 0.
        const exitCode = item.exitCode === 0 && failed ? 1 : item.exitCode;
        if (exitCode !== 0 && !failed) {
          const details = stderrTail.length ? ` :\n${stderrTail.join("\n")}` : ".";
          yield { type: "error", message: `« ${cmd} » s'est arrêté avec le code ${exitCode}${details}` };
        }
        yield { type: "done", exitCode, summary: summary ?? lastText };
        return;
      }
      if (!item.line.trim()) continue;
      if (item.stream === "stderr") {
        stderrTail.push(item.line);
        if (stderrTail.length > 5) stderrTail.shift();
      }
      const events: AgentEvent[] = item.stream === "stdout" ? parse(item.line) : [{ type: "log", message: item.line }];
      for (const event of events) {
        if (event.type === "done") {
          summary = event.summary;
          continue;
        }
        if (event.type === "text") lastText = event.text;
        if (event.type === "error") {
          // Certains agents répètent la même erreur (ex. Codex : `error` puis `turn.failed`).
          if (event.message === lastError) continue;
          lastError = event.message;
          failed = true;
        }
        yield event;
      }
    }
  } catch (err) {
    yield { type: "error", message: `Impossible de lancer « ${cmd} » : ${(err as Error).message}` };
    yield { type: "done", exitCode: 127 };
  }
}
