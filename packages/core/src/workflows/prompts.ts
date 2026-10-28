import { readFile } from "node:fs/promises";
import { join } from "node:path";

/** Prompt de la bibliothèque du projet (`.supacode/prompts/<id>.md`), comme dans l'IDE. */
export interface PromptFile {
  id: string;
  title: string;
  description?: string;
  agent?: string;
  body: string;
}

/** Sépare l'en-tête facultatif (`titre`, `description`, `agent`…) du corps du prompt. */
export function parsePrompt(id: string, text: string): PromptFile {
  const normalized = text.replace(/\r\n/g, "\n");
  const match = /^---\n([\s\S]*?)\n---\n?/.exec(normalized);
  const fields: Record<string, string> = {};
  for (const line of (match?.[1] ?? "").split("\n")) {
    const colon = line.indexOf(":");
    if (colon > 0) fields[line.slice(0, colon).trim().toLowerCase()] = line.slice(colon + 1).trim();
  }
  const body = (match ? normalized.slice(match[0].length) : normalized).replace(/^\n+/, "").trimEnd();
  const firstLine = body.split("\n").find((l) => l.trim())?.replace(/^#+\s*/, "").trim().slice(0, 60) ?? id;
  return { id, title: fields.titre || fields.title || firstLine, description: fields.description || undefined, agent: fields.agent || undefined, body };
}

export async function loadPrompt(projectDir: string, id: string): Promise<PromptFile> {
  if (!/^[\w-]+$/.test(id)) throw new Error(`nom de prompt invalide : « ${id} »`);
  try {
    return parsePrompt(id, await readFile(join(projectDir, ".supacode", "prompts", `${id}.md`), "utf8"));
  } catch {
    throw new Error(`prompt « ${id} » introuvable (.supacode/prompts/${id}.md)`);
  }
}
