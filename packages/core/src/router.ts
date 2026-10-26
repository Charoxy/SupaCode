import { classify } from "./classifier.js";
import type { SupaCodeConfig } from "./config.js";
import type { AgentId, RouteDecision } from "./types.js";

/** Choix manuel de l'utilisateur, prioritaire sur l'orchestrateur. */
export interface RouteOverride {
  agent?: AgentId;
  model?: string;
}

/** Décide quel agent et quel modèle doivent traiter la demande. */
export function route(prompt: string, config: SupaCodeConfig, override?: RouteOverride): RouteDecision {
  const classification = classify(prompt);

  if (override?.agent) {
    return {
      agent: override.agent,
      model: override.model,
      reason: "agent forcé manuellement",
      classification,
    };
  }

  const orchestrator = config.orchestrator;
  let decision: RouteDecision;
  if (!orchestrator.enabled) {
    decision = { ...orchestrator.fixed, reason: "orchestrateur désactivé → agent fixe", classification };
  } else {
    const { kind, size } = classification;
    const rule = orchestrator.rules.find((r) => (!r.kind || r.kind === kind) && (!r.size || r.size === size));
    decision = rule
      ? { agent: rule.agent, model: rule.model, reason: `${kind} / ${size} → règle ${rule.name}`, classification }
      : { ...orchestrator.fallback, reason: `${kind} / ${size} → repli`, classification };
  }

  if (override?.model) {
    decision = { ...decision, model: override.model, reason: `${decision.reason} (modèle forcé)` };
  }
  return decision;
}
