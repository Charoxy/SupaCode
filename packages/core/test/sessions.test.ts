import { mkdir, mkdtemp, utimes, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { listClaudeConversations, listClaudeProjects, parseSessionText } from "../src/index.js";

const jsonl = (...entries: unknown[]) => entries.map((e) => JSON.stringify(e)).join("\n");

describe("parseSessionText", () => {
  it("préfère le titre personnalisé, puis le titre généré, puis la dernière demande", () => {
    const user = { type: "user", cwd: "/p", message: { role: "user", content: "corrige le bug" } };
    expect(parseSessionText(jsonl(user)).title).toBe("corrige le bug");
    expect(parseSessionText(jsonl(user, { type: "last-prompt", lastPrompt: "ajoute un test" })).title).toBe("ajoute un test");
    expect(parseSessionText(jsonl(user, { type: "ai-title", aiTitle: "Bug du panier" })).title).toBe("Bug du panier");
    expect(parseSessionText(jsonl({ type: "custom-title", customTitle: "Mon titre" }, { type: "ai-title", aiTitle: "x" })).title).toBe("Mon titre");
    expect(parseSessionText(jsonl(user)).cwd).toBe("/p");
  });

  it("ignore les messages internes et les lignes coupées", () => {
    const meta = { type: "user", isMeta: true, message: { content: "méta" } };
    const command = { type: "user", message: { content: "<command-name>/clear</command-name>" } };
    const real = { type: "user", message: { content: [{ type: "text", text: "vrai message" }] } };
    expect(parseSessionText(`{"type":"user","cwd":"/coupé\n${jsonl(meta, command, real)}`).title).toBe("vrai message");
  });
});

describe("listClaudeProjects / listClaudeConversations", () => {
  it("liste les projets existants et leurs conversations, du plus récent au plus ancien", async () => {
    const root = await mkdtemp(join(tmpdir(), "supacode-claude-"));
    const projectDir = join(root, "projects", "mon-projet");
    await mkdir(projectDir, { recursive: true });
    const user = (text: string) => ({ type: "user", cwd: root, message: { content: text } });
    await writeFile(join(projectDir, "ancienne.jsonl"), jsonl(user("première")));
    await writeFile(join(projectDir, "recente.jsonl"), jsonl(user("seconde")));
    await writeFile(join(projectDir, "vide.jsonl"), jsonl({ type: "mode", mode: "x" }));
    await utimes(join(projectDir, "ancienne.jsonl"), new Date(1000), new Date(1000));
    // Projet dont le dossier n'existe plus : ignoré.
    await mkdir(join(root, "projects", "disparu"), { recursive: true });
    await writeFile(join(root, "projects", "disparu", "s.jsonl"), jsonl({ type: "user", cwd: join(root, "nope"), message: { content: "x" } }));

    const projects = await listClaudeProjects(join(root, "projects"));
    expect(projects.map((p) => p.id)).toEqual(["mon-projet"]);
    expect(projects[0]).toMatchObject({ path: root, conversationCount: 3 });

    const conversations = await listClaudeConversations("mon-projet", join(root, "projects"));
    expect(conversations.map((c) => [c.id, c.title])).toEqual([
      ["recente", "seconde"],
      ["ancienne", "première"],
    ]);
  });
});
