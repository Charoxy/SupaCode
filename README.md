# SupaCode

**Un IDE pour coder avec l'IA, qui choisit pour toi le bon agent et le bon modèle.**

Tu décris ce que tu veux. L'orchestrateur de SupaCode classe la demande (question, petite correction, refacto, nouvelle fonctionnalité, review) et estime sa taille. Il l'envoie ensuite à l'agent le plus adapté : [Claude Code](https://docs.anthropic.com/claude-code) (Haiku, Sonnet ou Opus) ou [Codex](https://github.com/openai/codex). Une petite typo part chez Codex, une refonte d'architecture chez Claude Opus. Plus tard, une application Android permettra de piloter le travail à distance.

## Ce qui marche aujourd'hui

- `@supacode/core` : classifieur de demandes (mots-clés français et anglais), routeur configurable, pilotage des CLI `claude` et `codex` avec un flux d'événements commun (texte, outils, erreurs, fin).
- `@supacode/cli` : la commande `supacode` pour voir la décision de routage (`route`) ou lancer la demande (`run`).
- `ide/` : le script qui récupère VSCodium et applique nos patchs (base de l'IDE à venir).
- Intégration continue GitHub Actions : build et tests à chaque push.

## Démarrage rapide

Prérequis : Node 20 ou plus. Pour `run`, il faut aussi la CLI `claude` et/ou `codex`, installée et connectée.

```bash
npm install
npm run build
npm test
```

Voir quel agent serait choisi :

```bash
$ node packages/cli/dist/index.js route "corrige la typo dans le README"
codex : small-fix / tiny → règle small-fix + tiny

$ node packages/cli/dist/index.js route "pourquoi le build est lent ?"
claude (haiku) : question / tiny → règle question

$ node packages/cli/dist/index.js route "refactorise toute l'architecture du projet"
claude (opus) : refactor / large → règle large
```

Lancer la demande dans le dossier courant :

```bash
node packages/cli/dist/index.js run "ajoute un test pour la fonction slugify"
node packages/cli/dist/index.js run "corrige la typo" --agent claude --model haiku
node packages/cli/dist/index.js run "explique ce module" --json   # événements bruts, un JSON par ligne
```

La sortie montre la décision (`▶`), le texte de l'agent, les outils utilisés (`· Edit src/app.ts`) et les erreurs (`✖`). La commande se termine avec le code de sortie de l'agent.

> Astuce : `npm link -w @supacode/cli` installe la commande `supacode` globalement.

## Configuration

Place un fichier `.supacode/config.json` à la racine de ton projet. Il est fusionné avec la configuration par défaut : tu peux n'indiquer que ce qui change.

```json
{
  "orchestrator": {
    "enabled": true,
    "fixed": { "agent": "claude", "model": "sonnet" },
    "rules": [
      { "name": "small-fix + tiny", "kind": "small-fix", "size": "tiny", "agent": "codex" },
      { "name": "question", "kind": "question", "agent": "claude", "model": "haiku" },
      { "name": "review", "kind": "review", "agent": "claude", "model": "sonnet" },
      { "name": "large", "size": "large", "agent": "claude", "model": "opus" },
      { "name": "refactor", "kind": "refactor", "agent": "claude", "model": "opus" },
      { "name": "tiny", "size": "tiny", "agent": "codex" }
    ],
    "fallback": { "agent": "claude", "model": "sonnet" }
  }
}
```

- Les règles sont évaluées dans l'ordre et la première qui correspond gagne. `kind` et `size` sont facultatifs.
- `"enabled": false` envoie toutes les demandes vers `fixed`.
- Les modèles Claude sont les alias de la CLI Claude Code (`haiku`, `sonnet`, `opus`).
- `--agent` et `--model` passés à la CLI sont toujours prioritaires.

## Structure

| Chemin | Rôle |
| --- | --- |
| `packages/core` | `@supacode/core` : types, classifieur, configuration, routeur, agents Claude/Codex, orchestrateur |
| `packages/cli` | `@supacode/cli` : commande `supacode` (`route`, `run`) |
| `ide/` | Script de récupération de VSCodium et patchs SupaCode |
| `docs/` | Documentation ([architecture et feuille de route](docs/architecture.md)) |
| `.github/workflows` | Intégration continue |

## Licence

[MIT](LICENSE) © Charoxy
