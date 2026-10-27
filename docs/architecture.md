# Architecture de SupaCode

## Vue d'ensemble

```
 ┌────────────────┐   ┌────────────────┐   ┌────────────────┐
 │ IDE SupaCode   │   │ APK Android    │   │ CLI supacode   │
 │ (natif, Rust   │   │ (pilotage à    │   │ (route, run,   │
 │  + GPUI)       │   │  distance)     │   │  workflow)     │
 └───────┬────────┘   └───────┬────────┘   └───────┬────────┘
         │                    │                    │
         └────────────────────┼────────────────────┘
                              ▼
       ┌───────────────────────────────────────┐
       │            @supacode/core             │
       │                                       │
       │  classify ─► route ─► Orchestrator    │
       │  runWorkflow ─────────────┘           │
       │                            │          │
       │              ┌─────────────┴───┐      │
       │              ▼                 ▼      │
       │         ClaudeAgent       CodexAgent  │
       └──────────────┬─────────────────┬──────┘
                      ▼                 ▼
               CLI `claude -p`    CLI `codex exec`
               (stream-json)      (--json)
```

Le cœur existe en deux versions qui suivent les mêmes règles et lisent les mêmes fichiers (`.supacode/config.json`, `.supacode/workflows/*.json`) : `@supacode/core` en TypeScript pour la CLI, et `supacode-core` en Rust pour l'IDE.

Le cœur ne parle à aucune API directement. Il pilote les CLI officielles, déjà installées et connectées par l'utilisateur, puis convertit leur sortie en un flux d'événements commun. L'IDE, la CLI et plus tard l'APK consomment ce même flux.

## Modules de `@supacode/core`

| Module | Rôle |
| --- | --- |
| `types.ts` | Types partagés : `AgentId`, `TaskKind`, `TaskSize`, `Classification`, `RouteDecision`, `AgentRequest`, `AgentEvent` et l'interface `Agent`. |
| `classifier.ts` | `classify(prompt)` : déduit le type (question, small-fix, refactor, feature, review) et la taille (tiny, medium, large) à partir de mots-clés français et anglais et de la longueur. Les `signals` expliquent le choix. |
| `config.ts` | `SupaCodeConfig`, `DEFAULT_CONFIG` et `loadConfig(projectDir)`, qui fusionne `.supacode/config.json` avec les valeurs par défaut. |
| `router.ts` | `route(prompt, config, override?)` : agent forcé, sinon agent fixe si l'orchestrateur est désactivé, sinon la première règle qui correspond, sinon le repli. Produit une raison lisible. |
| `agents/process.ts` | `spawnLines` (lignes stdout/stderr d'un processus, puis code de sortie), `commandExists`, et `streamAgent`, qui transforme une CLI en flux d'`AgentEvent`. |
| `agents/claude.ts` | `ClaudeAgent` (`claude -p … --output-format stream-json`) et `parseClaudeLine`. |
| `agents/codex.ts` | `CodexAgent` (`codex exec --json`) et `parseCodexLine`. |
| `orchestrator.ts` | `Orchestrator` : `decide()` pour la décision seule, `run()` pour émettre la décision, vérifier que l'agent est installé, puis relayer ses événements. |
| `workflows/types.ts` | Format des workflows (`Workflow`, étapes `run` / `prompt` / `approve` / `parallel`, déclencheurs) et `WorkflowEvent`. |
| `workflows/loader.ts` | `parseWorkflow` (validation avec messages lisibles), `loadWorkflows(projectDir)` (lit `.supacode/workflows/*.json`), `workflowsFor(trigger)`. |
| `workflows/engine.ts` | `runWorkflow(workflow, orchestrator, options)` : exécute les étapes, gère conditions, parallélisme, validations, variables `{{…}}` et annulation. |

### Le flux d'événements

Chaque exécution émet d'abord un événement `route` (la décision), puis des `text`, `tool` et `log`, avec éventuellement des `error`. Elle se termine toujours par un `done` qui porte le code de sortie de l'agent. Un client (CLI, panneau de l'IDE, APK) n'a donc qu'un seul format à afficher.

## Workflows

Un workflow est un fichier `.supacode/workflows/<id>.json` versionné avec le projet. L'IDE, la CLI et plus tard l'APK l'exécutent avec le même moteur.

```json
{
  "name": "Vérifs au commit",
  "on": ["commit"],
  "steps": [
    { "id": "lint", "run": "npm run lint" },
    { "id": "fix", "if": "steps.lint.failed", "agent": "codex", "prompt": "Corrige ces erreurs :\n{{steps.lint.output}}" },
    { "id": "checks", "parallel": [
      { "id": "tests", "run": "npm test" },
      { "id": "review", "agent": "auto", "prompt": "Relis ce diff :\n{{git.diff}}" }
    ]},
    { "id": "ok", "approve": "Valider le commit ?" }
  ]
}
```

- **Déclencheurs** (`on`) : `manual` (défaut), `commit` et `push` (via `supacode hooks install`), `{ "save": "<glob>" }` (dans l'IDE).
- **Étapes** : `run` (commande shell), `prompt` (agent ; `agent: "auto"` laisse l'orchestrateur choisir), `approve` (validation humaine), `parallel` (étapes simultanées).
- **Conditions** (`if`) : `always` ou `steps.<id>.success|failed|skipped`. Sans condition, une étape ne s'exécute que si aucune étape précédente n'a échoué. Un échec rattrapé par une étape `if: "steps.<id>.failed"` ne fait pas échouer le workflow.
- **Variables** dans les prompts : `{{git.diff}}`, `{{git.branch}}`, `{{steps.<id>.output}}`, `{{steps.<id>.status}}` et le contexte du client (`{{file}}` pour le déclencheur `save`, par exemple). Les commandes `run` reçoivent ces valeurs en variables d'environnement `SUPACODE_*` (ex. `SUPACODE_STEP_LINT_OUTPUT`), pour éviter toute injection dans le shell.
- **Événements** : `workflow.start`, `step.start`, `step.event` (qui enveloppe un `AgentEvent`), `approval.request`, `step.done`, `workflow.done`.

## L'IDE

L'IDE est une application native en Rust, construite sur GPUI (le moteur d'interface de Zed, via `gpui-kit`).

| Crate / module | Rôle |
| --- | --- |
| `supacode-core::claude` | Projets et conversations de Claude Code (`~/.claude/projects/<projet>/<session>.jsonl`), titres et dates. |
| `supacode-core::router` | Classifieur et routeur, identiques à ceux de `@supacode/core`. |
| `supacode-core::agents` | `claude -p` (stream-json) et `codex exec --json` ; la demande passe par l'entrée standard. |
| `supacode-core::engine` | Moteur de workflows : conditions, parallélisme, validations, `{{variables}}`, `SUPACODE_*`, arrêt. |
| `supacode-core::process` | Lancement des commandes, lecture ligne à ligne, arrêt de l'arbre de processus. |
| `supacode-app::terminal` | Terminal intégré (`alacritty_terminal`, ConPTY sous Windows) où tourne la CLI `claude`. |
| `supacode-app::preview` | Surveillance du projet et diff des fichiers modifiés par rapport à `HEAD`. |
| `supacode-app::workspace` | La fenêtre : projets et conversations, onglets, aperçu du code, suivi des workflows (`workspace/runs.rs`). |

Une nouvelle conversation est lancée avec `claude --session-id <uuid>` : l'IDE connaît son identifiant dès le départ et affiche son vrai titre dès que Claude Code l'enregistre. Les étapes d'agent des workflows utilisent `--no-session-persistence` pour ne pas encombrer la liste des conversations.

## Feuille de route

1. **Fondations** *(fait)* : monorepo TypeScript, cœur avec classifieur, routeur et agents Claude/Codex, CLI `supacode`, CI.
2. **Moteur de workflows et IDE** *(en cours)* : moteur de workflows dans le cœur, CLI `workflow` et hooks git *(fait)*. IDE natif en Rust : projets et conversations, terminal `claude`, aperçu du code, lancement et suivi des workflows avec validations *(fait)* ; palette Ctrl+K, déclencheur à l'enregistrement, coloration syntaxique *(à venir)*.
3. **Un vrai IDE IA** : éditeur visuel de workflows, création de workflows en langage naturel, diff et application des modifications dans le panneau, historique des exécutions, bibliothèque de workflows prêts à l'emploi, paquets d'installation.
4. **APK Android** : suivre et piloter le travail à distance (lancer une demande ou un workflow, voir le flux d'événements, valider ou arrêter), grâce à un serveur exposé par le cœur.
