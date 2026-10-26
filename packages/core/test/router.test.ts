import { describe, expect, it } from "vitest";
import { DEFAULT_CONFIG, mergeConfig, route } from "../src/index.js";

describe("route", () => {
  it("envoie un petit fix vers codex", () => {
    const d = route("fix the typo in the header", DEFAULT_CONFIG);
    expect(d.agent).toBe("codex");
    expect(d.reason).toBe("small-fix / tiny → règle small-fix + tiny");
  });

  it("envoie une refacto longue vers claude opus", () => {
    const d = route(
      "Refactorise le module de paiement pour séparer la validation, le calcul des taxes et l'appel au prestataire dans des fonctions distinctes",
      DEFAULT_CONFIG,
    );
    expect(d).toMatchObject({ agent: "claude", model: "opus", reason: "refactor / medium → règle refactor" });
  });

  it("respecte un agent forcé manuellement", () => {
    const d = route("corrige la typo", DEFAULT_CONFIG, { agent: "claude", model: "opus" });
    expect(d).toMatchObject({ agent: "claude", model: "opus", reason: "agent forcé manuellement" });
  });

  it("utilise l'agent fixe quand l'orchestrateur est désactivé", () => {
    const config = mergeConfig({ orchestrator: { enabled: false, fixed: { agent: "codex", model: "gpt-5" } } });
    const d = route("Refactorise toute l'architecture", config);
    expect(d).toMatchObject({ agent: "codex", model: "gpt-5" });
  });

  it("se replie sur claude sonnet sans règle applicable", () => {
    const d = route("un tableau de bord avec les ventes du mois par région et par vendeur", DEFAULT_CONFIG);
    expect(d).toMatchObject({ agent: "claude", model: "sonnet", reason: "feature / medium → repli" });
  });
});
