import { readFile } from "node:fs/promises";
import { join } from "node:path";
import type { AgentId, TaskKind, TaskSize } from "./types.js";

/** Agent + modèle (alias de la CLI, ex. « sonnet » pour Claude Code). */
export interface AgentChoice {
  agent: AgentId;
  model?: string;
}

/** Règle de routage : s'applique si `kind` et `size` (facultatifs) correspondent. */
export interface RouteRule extends AgentChoice {
  name: string;
  kind?: TaskKind;
  size?: TaskSize;
}

export interface OrchestratorConfig {
  /** Si faux, toutes les demandes partent vers `fixed`. */
  enabled: boolean;
  fixed: AgentChoice;
  /** Évaluées dans l'ordre : la première qui correspond gagne. */
  rules: RouteRule[];
  /** Utilisé quand aucune règle ne correspond. */
  fallback: AgentChoice;
}

export interface SupaCodeConfig {
  orchestrator: OrchestratorConfig;
}

export const DEFAULT_CONFIG: SupaCodeConfig = {
  orchestrator: {
    enabled: true,
    fixed: { agent: "claude", model: "sonnet" },
    rules: [
      { name: "small-fix + tiny", kind: "small-fix", size: "tiny", agent: "codex" },
      { name: "question", kind: "question", agent: "claude", model: "haiku" },
      { name: "review", kind: "review", agent: "claude", model: "sonnet" },
      { name: "large", size: "large", agent: "claude", model: "opus" },
      { name: "refactor", kind: "refactor", agent: "claude", model: "opus" },
      { name: "tiny", size: "tiny", agent: "codex" },
    ],
    fallback: { agent: "claude", model: "sonnet" },
  },
};

export const CONFIG_PATH = join(".supacode", "config.json");

/** Fusionne une config partielle (lue sur disque) avec la config par défaut. */
export function mergeConfig(partial: unknown): SupaCodeConfig {
  const base = DEFAULT_CONFIG.orchestrator;
  const user = (isObject(partial) && isObject(partial.orchestrator) ? partial.orchestrator : {}) as Partial<OrchestratorConfig>;
  return {
    orchestrator: {
      enabled: user.enabled ?? base.enabled,
      fixed: { ...base.fixed, ...user.fixed },
      rules: Array.isArray(user.rules) ? user.rules : base.rules,
      fallback: { ...base.fallback, ...user.fallback },
    },
  };
}

/** Lit `.supacode/config.json` dans `projectDir` ; config par défaut si le fichier n'existe pas. */
export async function loadConfig(projectDir: string): Promise<SupaCodeConfig> {
  const path = join(projectDir, CONFIG_PATH);
  let raw: string;
  try {
    raw = await readFile(path, "utf8");
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") return mergeConfig({});
    throw err;
  }
  try {
    return mergeConfig(JSON.parse(raw));
  } catch (err) {
    throw new Error(`Configuration invalide dans ${path} : ${(err as Error).message}`);
  }
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
