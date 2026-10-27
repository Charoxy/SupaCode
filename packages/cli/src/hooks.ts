import { execFileSync } from "node:child_process";
import { chmod, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** Marqueur qui identifie les hooks écrits par SupaCode (les autres ne sont jamais touchés). */
const MARKER = "# supacode-hook";
const HOOKS = { "pre-commit": "commit", "pre-push": "push" } as const;

function hooksDir(cwd: string): string {
  // `--git-path hooks` respecte core.hooksPath et les worktrees.
  const path = execFileSync("git", ["rev-parse", "--git-path", "hooks"], { cwd, encoding: "utf8" }).trim();
  return resolve(cwd, path);
}

export async function installHooks(cwd: string): Promise<number> {
  const dir = hooksDir(cwd);
  await mkdir(dir, { recursive: true });
  const cli = fileURLToPath(new URL("./index.js", import.meta.url)).replace(/\\/g, "/");

  let code = 0;
  for (const [hook, trigger] of Object.entries(HOOKS)) {
    const path = join(dir, hook);
    const existing = await readFile(path, "utf8").catch(() => undefined);
    if (existing !== undefined && !existing.includes(MARKER)) {
      console.error(`✖ ${hook} existe déjà et n'a pas été créé par SupaCode : laissé tel quel.`);
      code = 1;
      continue;
    }
    await writeFile(path, `#!/bin/sh\n${MARKER}\nexec node "${cli}" workflow trigger ${trigger}\n`);
    await chmod(path, 0o755);
    console.log(`✔ ${hook} → workflows « ${trigger} »`);
  }
  return code;
}

export async function uninstallHooks(cwd: string): Promise<number> {
  const dir = hooksDir(cwd);
  for (const hook of Object.keys(HOOKS)) {
    const path = join(dir, hook);
    const existing = await readFile(path, "utf8").catch(() => undefined);
    if (existing?.includes(MARKER)) {
      await rm(path);
      console.log(`✔ ${hook} retiré`);
    }
  }
  return 0;
}
