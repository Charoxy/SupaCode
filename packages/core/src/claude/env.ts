/**
 * Variables qui identifient une session Claude Code en cours. Si SupaCode est lancé
 * depuis Claude Code, ses enfants en héritent : un `claude` lancé ensuite se croirait
 * sous-session (historique non enregistré, mauvais canal de messages…). On les retire donc.
 * Les réglages de l'utilisateur (ex. CLAUDE_CODE_USE_BEDROCK) sont conservés.
 */
export const CLAUDE_SESSION_ENV = [
  "CLAUDECODE",
  "CLAUDE_PID",
  "CLAUDE_EFFORT",
  "CLAUDE_CODE_ENTRYPOINT",
  "CLAUDE_CODE_EXECPATH",
  "CLAUDE_CODE_SESSION_ID",
  "CLAUDE_CODE_CHILD_SESSION",
  "CLAUDE_CODE_SESSION_ATTENDED",
  "CLAUDE_CODE_BRIDGE_SESSION_ID",
  "CLAUDE_CODE_MESSAGING_SOCKET",
  "CLAUDE_CODE_MESSAGING_TOKEN",
  "CLAUDE_CODE_SSE_PORT",
] as const;

/** Copie de `env` sans les marqueurs de session Claude Code. */
export function withoutClaudeSession(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const clean = { ...env };
  for (const key of CLAUDE_SESSION_ENV) delete clean[key];
  return clean;
}
