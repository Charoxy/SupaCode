import { existsSync } from "node:fs";
import { open, readdir, stat } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, join } from "node:path";

/**
 * Lecture des conversations de Claude Code (`~/.claude/projects/<projet>/<session>.jsonl`),
 * pour lister les projets et leurs conversations, puis les reprendre avec `claude --resume <id>`.
 */

export interface ClaudeProject {
  /** Nom du dossier dans `~/.claude/projects` (chemin encodé par Claude Code). */
  id: string;
  /** Dossier du projet sur le disque. */
  path: string;
  name: string;
  /** Date de la dernière activité (ms). */
  lastActivity: number;
  conversationCount: number;
}

export interface ClaudeConversation {
  /** Identifiant de session, à passer à `claude --resume`. */
  id: string;
  projectId: string;
  title: string;
  lastActivity: number;
}

/** Octets lus au début et à la fin de chaque fichier : les titres et le dossier s'y trouvent. */
const HEAD_BYTES = 64 * 1024;
const TAIL_BYTES = 256 * 1024;

export function claudeProjectsDir(claudeHome = process.env.CLAUDE_CONFIG_DIR ?? join(homedir(), ".claude")): string {
  return join(claudeHome, "projects");
}

/** Projets ayant au moins une conversation, dont le dossier existe encore, du plus récent au plus ancien. */
export async function listClaudeProjects(projectsDir = claudeProjectsDir()): Promise<ClaudeProject[]> {
  const entries = await readdir(projectsDir, { withFileTypes: true }).catch(() => []);
  const projects = await Promise.all(
    entries.filter((e) => e.isDirectory()).map(async (entry): Promise<ClaudeProject | undefined> => {
      const sessions = await sessionFiles(join(projectsDir, entry.name));
      const latest = sessions[0];
      if (!latest) return undefined;
      let cwd: string | undefined;
      // Une session vide n'a pas de dossier : on regarde les suivantes.
      for (const session of sessions.slice(0, 5)) {
        cwd = (await readChunks(session.file)).cwd;
        if (cwd) break;
      }
      const path = cwd ?? decodeProjectId(entry.name);
      if (!existsSync(path)) return undefined;
      return { id: entry.name, path, name: basename(path) || path, lastActivity: latest.mtime, conversationCount: sessions.length };
    }),
  );
  return projects.filter((p): p is ClaudeProject => p !== undefined).sort((a, b) => b.lastActivity - a.lastActivity);
}

/** Conversations d'un projet, de la plus récente à la plus ancienne (les conversations vides sont ignorées). */
export async function listClaudeConversations(projectId: string, projectsDir = claudeProjectsDir(), limit = 50): Promise<ClaudeConversation[]> {
  const sessions = (await sessionFiles(join(projectsDir, projectId))).slice(0, limit);
  const conversations = await Promise.all(
    sessions.map(async ({ file, mtime }): Promise<ClaudeConversation | undefined> => {
      const info = await readChunks(file);
      if (!info.title) return undefined;
      return { id: basename(file, ".jsonl"), projectId, title: info.title, lastActivity: mtime };
    }),
  );
  return conversations.filter((c): c is ClaudeConversation => c !== undefined);
}

async function sessionFiles(dir: string): Promise<Array<{ file: string; mtime: number }>> {
  const names = await readdir(dir).catch(() => [] as string[]);
  const files = await Promise.all(
    names.filter((n) => n.endsWith(".jsonl")).map(async (n) => {
      const file = join(dir, n);
      const info = await stat(file).catch(() => undefined);
      return info && info.size > 0 ? { file, mtime: info.mtimeMs } : undefined;
    }),
  );
  return files.filter((f): f is { file: string; mtime: number } => f !== undefined).sort((a, b) => b.mtime - a.mtime);
}

interface SessionInfo {
  cwd?: string;
  title?: string;
}

/** Lit le début et la fin d'un fichier de session et en extrait dossier et titre. */
async function readChunks(file: string): Promise<SessionInfo> {
  const handle = await open(file, "r");
  try {
    const { size } = await handle.stat();
    const head = Buffer.alloc(Math.min(HEAD_BYTES, size));
    await handle.read(head, 0, head.length, 0);
    let text = head.toString("utf8");
    if (size > HEAD_BYTES) {
      const length = Math.min(TAIL_BYTES, size - HEAD_BYTES);
      const tail = Buffer.alloc(length);
      await handle.read(tail, 0, length, size - length);
      text += `\n${tail.toString("utf8")}`;
    }
    return parseSessionText(text);
  } finally {
    await handle.close();
  }
}

/**
 * Titre par ordre de préférence : titre donné par l'utilisateur, titre généré,
 * dernière demande, sinon premier message de l'utilisateur.
 */
export function parseSessionText(text: string): SessionInfo {
  let cwd: string | undefined;
  let custom: string | undefined;
  let ai: string | undefined;
  let lastPrompt: string | undefined;
  let firstUser: string | undefined;

  for (const line of text.split("\n")) {
    // Filtre rapide : on ne parse que les lignes utiles (les fichiers peuvent être gros).
    if (!line.startsWith("{") || !/"(cwd|customTitle|aiTitle|lastPrompt)"|"type":"user"/.test(line)) continue;
    let entry: Record<string, unknown>;
    try {
      entry = JSON.parse(line) as Record<string, unknown>;
    } catch {
      continue; // Ligne coupée par la lecture partielle.
    }
    if (!cwd && typeof entry.cwd === "string") cwd = entry.cwd;
    if (entry.type === "custom-title" && typeof entry.customTitle === "string") custom = entry.customTitle;
    if (entry.type === "ai-title" && typeof entry.aiTitle === "string") ai = entry.aiTitle;
    if (entry.type === "last-prompt" && typeof entry.lastPrompt === "string") lastPrompt = entry.lastPrompt;
    if (!firstUser && entry.type === "user" && !entry.isMeta && !entry.isSidechain) firstUser = userText(entry.message);
  }

  const title = [custom, ai, lastPrompt, firstUser].find((t) => t?.trim())?.trim();
  return { cwd, title: title ? oneLine(title) : undefined };
}

function userText(message: unknown): string | undefined {
  const content = (message as { content?: unknown } | undefined)?.content;
  const text =
    typeof content === "string"
      ? content
      : Array.isArray(content)
        ? (content as Array<{ type?: string; text?: string }>).find((b) => b.type === "text")?.text
        : undefined;
  // Les commandes et sorties internes (<command-name>, <local-command-stdout>…) ne font pas un bon titre.
  return text && !text.trimStart().startsWith("<") ? text : undefined;
}

function oneLine(text: string): string {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > 80 ? `${line.slice(0, 79)}…` : line;
}

/** Repli si aucune ligne ne donne le dossier : « G--workspace-app » → « G:\workspace\app » (approximatif). */
function decodeProjectId(id: string): string {
  const windows = /^([A-Za-z])--(.*)$/.exec(id);
  if (windows) return `${windows[1]}:\\${windows[2]!.replace(/-/g, "\\")}`;
  return id.replace(/-/g, "/");
}
