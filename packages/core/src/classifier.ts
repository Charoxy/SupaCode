import type { Classification, TaskKind, TaskSize } from "./types.js";

// Limites de mot tolérantes aux accents (\b ne connaît pas « é »).
const START = "(?<![\p{L}\p{N}_])";
const END = "(?![\p{L}\p{N}_])";

interface Keyword {
  label: string;
  pattern: RegExp;
}

/** Mot exact, ou préfixe si `prefix` (ex. « refacto » couvre « refactoriser »). */
function kw(label: string, source: string, prefix = false): Keyword {
  return { label, pattern: new RegExp(`${START}(?:${source})${prefix ? "" : END}`, "iu") };
}

// Ordre = priorité : le premier type qui a un indice l'emporte.
const KIND_KEYWORDS: Array<[TaskKind, Keyword[]]> = [
  ["review", [kw("review", "review", true), kw("relis", "reli(?:s|re|sez)|relecture", true), kw("audit", "audit", true)]],
  [
    "question",
    [
      kw("pourquoi", "pourquoi"),
      kw("comment", "comment"),
      kw("explique", "expliqu", true),
      kw("why", "why"),
      kw("how", "how"),
      kw("what", "what"),
      kw("explain", "explain", true),
    ],
  ],
  [
    "small-fix",
    [
      kw("typo", "typos?"),
      kw("fix", "fix(?:e[sd]?|ing)?"),
      kw("corrige", "corrig", true),
      kw("renomme", "renomm", true),
      kw("rename", "renam", true),
    ],
  ],
  [
    "refactor",
    [
      kw("refacto", "refacto|refactor", true),
      kw("nettoie", "nettoi|nettoy", true),
      kw("simplifie", "simplifi", true),
      kw("clean up", "clean ?up"),
      kw("simplify", "simplify"),
    ],
  ],
  [
    "feature",
    [
      kw("ajoute", "ajout", true),
      kw("crée", "cr[ée]{2}", true),
      kw("implémente", "impl[ée]ment", true),
      kw("add", "add"),
      kw("create", "create"),
      kw("implement", "implement"),
    ],
  ],
];

const LARGE_KEYWORDS: Keyword[] = [
  kw("tout le projet", "tout(?:e)? (?:le|la|l['’]) ?(?:projet|app|application|code|base de code|codebase|repo|dépôt)"),
  kw("whole project", "(?:whole|entire) (?:project|app|codebase|repo)"),
  kw("architecture", "architectur", true),
  kw("migration", "migr", true),
];

const TINY_KEYWORDS: Keyword[] = [kw("juste", "juste|just"), kw("simple", "simple"), kw("petit", "petite?s?"), kw("typo", "typos?")];

const LARGE_WORDS = 120;
const TINY_WORDS = 12;

function firstMatch(text: string, keywords: Keyword[]): Keyword | undefined {
  return keywords.find((k) => k.pattern.test(text));
}

/** Classe une demande (type + taille) à partir de mots-clés français et anglais. */
export function classify(prompt: string): Classification {
  const text = prompt.trim();
  const signals: string[] = [];

  let kind: TaskKind = "feature";
  let kindFound = false;
  for (const [candidate, keywords] of KIND_KEYWORDS) {
    const hit = firstMatch(text, keywords);
    if (hit) {
      kind = candidate;
      kindFound = true;
      signals.push(`mot-clé « ${hit.label} » → ${candidate}`);
      break;
    }
    if (candidate === "question" && text.endsWith("?")) {
      kind = "question";
      kindFound = true;
      signals.push("se termine par « ? » → question");
      break;
    }
  }
  if (!kindFound) signals.push("aucun mot-clé → feature par défaut");

  const words = text.split(/\s+/).filter(Boolean).length;
  let size: TaskSize = "medium";
  const large = firstMatch(text, LARGE_KEYWORDS);
  const tiny = firstMatch(text, TINY_KEYWORDS);
  if (large) {
    size = "large";
    signals.push(`mot-clé « ${large.label} » → large`);
  } else if (words > LARGE_WORDS) {
    size = "large";
    signals.push(`${words} mots (> ${LARGE_WORDS}) → large`);
  } else if (tiny) {
    size = "tiny";
    signals.push(`mot-clé « ${tiny.label} » → tiny`);
  } else if (kind === "small-fix") {
    size = "tiny";
    signals.push("petite correction → tiny");
  } else if (words < TINY_WORDS) {
    size = "tiny";
    signals.push(`${words} mots (< ${TINY_WORDS}) → tiny`);
  } else {
    signals.push(`${words} mots → medium`);
  }

  return { kind, size, signals };
}
