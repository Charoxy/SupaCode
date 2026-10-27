import type { AgentEvent, AgentId } from "../types.js";

/**
 * Moment où un workflow se lance :
 * - "manual" : à la demande (palette, raccourci, CLI, APK) ;
 * - "commit" / "push" : via les hooks git installés par SupaCode ;
 * - { save: "<glob>" } : à l'enregistrement d'un fichier qui correspond au motif.
 */
export type WorkflowTrigger = "manual" | "commit" | "push" | { save: string };

interface StepBase {
  /** Identifiant unique dans le workflow, utilisé dans `{{steps.<id>.output}}`. */
  id: string;
  /** Libellé affiché (par défaut : l'id). */
  name?: string;
  /**
   * Condition d'exécution : "always", ou "steps.<id>.success|failed|skipped".
   * Sans condition, l'étape ne s'exécute que si aucune étape précédente n'a échoué
   * (un échec rattrapé par une étape `if: "steps.<id>.failed"` ne compte pas).
   */
  if?: string;
}

/** Commande shell (linters, tests, build…). */
export interface RunStep extends StepBase {
  run: string;
}

/** Tâche confiée à un agent ; "auto" (défaut) laisse l'orchestrateur choisir. */
export interface AgentStep extends StepBase {
  prompt: string;
  agent?: AgentId | "auto";
  model?: string;
}

/** Point de validation humaine (IDE ou APK). */
export interface ApproveStep extends StepBase {
  approve: string;
}

/** Étapes lancées en même temps ; échoue si l'une d'elles échoue. */
export interface ParallelStep extends StepBase {
  parallel: WorkflowStep[];
}

export type WorkflowStep = RunStep | AgentStep | ApproveStep | ParallelStep;
export type StepKind = "run" | "agent" | "approve" | "parallel";

/** Contenu d'un fichier `.supacode/workflows/<id>.json`. */
export interface Workflow {
  name: string;
  description?: string;
  /** Déclencheurs ; ["manual"] par défaut. */
  on?: WorkflowTrigger[];
  steps: WorkflowStep[];
}

/** Workflow chargé depuis le disque. */
export interface WorkflowFile {
  /** Nom du fichier sans extension. */
  id: string;
  path: string;
  workflow: Workflow;
}

export type StepStatus = "success" | "failed" | "skipped";
export type WorkflowStatus = "success" | "failed" | "cancelled";

/** Événements d'une exécution de workflow (consommés par la CLI, l'IDE et l'APK). */
export type WorkflowEvent =
  | { type: "workflow.start"; runId: string; workflow: string }
  | { type: "step.start"; stepId: string; kind: StepKind; name: string }
  /** Événement de l'agent (ou ligne de sortie d'une commande, sous forme de `log`). */
  | { type: "step.event"; stepId: string; event: AgentEvent }
  | { type: "approval.request"; stepId: string; message: string }
  | { type: "step.done"; stepId: string; status: StepStatus; exitCode?: number }
  | { type: "workflow.done"; runId: string; status: WorkflowStatus };

export interface WorkflowRunOptions {
  cwd: string;
  signal?: AbortSignal;
  /**
   * Variables supplémentaires injectées dans les prompts et commandes,
   * ex. { selection: "...", file: "src/app.ts" } → `{{selection}}`, `{{file}}`.
   */
  context?: Record<string, string>;
  /** Appelée pour chaque étape `approve` ; refus ou absence → l'étape échoue. */
  approve?: (stepId: string, message: string) => Promise<boolean>;
}
