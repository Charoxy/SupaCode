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

  const orchestrator = config.orchestrator;
  const { kind, size } = classification;
  const matches = (r: { kind?: string; size?: string }) => (!r.kind || r.kind === kind) && (!r.size || r.size === size);
  let decision: RouteDecision;
  if (override?.agent) {
    // Agent imposé : le modèle reste choisi par l'orchestrateur (règle qui vise cet agent, sinon repli).
    const agent = override.agent;
    const rule = orchestrator.enabled ? orchestrator.rules.find((r) => r.agent === agent && matches(r)) : undefined;
    const base = orchestrator.enabled ? orchestrator.fallback : orchestrator.fixed;
    const model = rule ? rule.model : base.agent === agent ? base.model : undefined;
    const reason = rule ? `${kind} / ${size} → ${agent} imposé, modèle de la règle ${rule.name}` : `${kind} / ${size} → ${agent} imposé`;
    decision = model !== undefined ? { agent, model, reason, classification } : { agent, reason, classification };
  } else if (!orchestrator.enabled) {
    decision = { ...orchestrator.fixed, reason: "orchestrateur désactivé → agent fixe", classification };
  } else {
    const rule = orchestrator.rules.find(matches);
    decision = rule
      ? { agent: rule.agent, model: rule.model, reason: `${kind} / ${size} → règle ${rule.name}`, classification }
      : { ...orchestrator.fallback, reason: `${kind} / ${size} → repli`, classification };
  }

  if (override?.model) {
    decision = { ...decision, model: override.model, reason: `${decision.reason} (modèle forcé)` };
  }
  return decision;
}
