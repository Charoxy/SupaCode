/** Agents de code pilotés par SupaCode. */
export type AgentId = "claude" | "codex";

/** Nature de la demande. */
export type TaskKind = "question" | "small-fix" | "refactor" | "feature" | "review";

/** Ampleur estimée de la demande. */
export type TaskSize = "tiny" | "medium" | "large";

/** Résultat du classifieur. */
export interface Classification {
  kind: TaskKind;
  size: TaskSize;
  /** Indices lisibles qui ont mené au choix du type et de la taille. */
  signals: string[];
}

/** Décision de l'orchestrateur : quel agent, quel modèle, et pourquoi. */
export interface RouteDecision {
  agent: AgentId;
  model?: string;
  reason: string;
  classification?: Classification;
}

/** Ce qu'on transmet à un agent pour une exécution. */
export interface AgentRequest {
  prompt: string;
  cwd: string;
  model?: string;
}

/** Événements émis pendant une exécution. */
export type AgentEvent =
  | { type: "route"; decision: RouteDecision }
  | { type: "text"; text: string }
  | { type: "tool"; name: string; detail?: string }
  | { type: "log"; message: string }
  | { type: "done"; exitCode: number; summary?: string }
  | { type: "error"; message: string };

/** Un agent de code (CLI externe) capable d'exécuter une demande. */
export interface Agent {
  readonly id: AgentId;
  /** Vrai si la CLI de l'agent est installée et répond. */
  isAvailable(): Promise<boolean>;
  /** Lance l'agent ; le flux se termine toujours par un événement `done`. */
  run(request: AgentRequest, signal?: AbortSignal): AsyncIterable<AgentEvent>;
}
