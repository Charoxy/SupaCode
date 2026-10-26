import { describe, expect, it } from "vitest";
import { classify } from "../src/index.js";

describe("classify", () => {
  it("classe une typo en small-fix / tiny", () => {
    const c = classify("corrige la typo dans le README");
    expect(c.kind).toBe("small-fix");
    expect(c.size).toBe("tiny");
    expect(c.signals.length).toBeGreaterThan(0);
  });

  it("reconnaît une question qui finit par « ? »", () => {
    const c = classify("Est-ce que ce module gère les erreurs réseau ?");
    expect(c.kind).toBe("question");
    expect(c.signals).toContain("se termine par « ? » → question");
  });

  it("classe une migration de toute l'app en large", () => {
    const c = classify("Passe toute l'app sur la nouvelle API et prépare la migration de la base");
    expect(c.size).toBe("large");
  });

  it("utilise feature par défaut", () => {
    expect(classify("un tableau de bord avec les ventes du mois par région et par vendeur").kind).toBe("feature");
  });

  it("reconnaît une review", () => {
    expect(classify("relis mon dernier commit").kind).toBe("review");
  });
});
