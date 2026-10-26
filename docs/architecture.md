# Architecture de SupaCode

## Vue d'ensemble

```
 ┌────────────────┐   ┌────────────────┐   ┌────────────────┐
 │ IDE SupaCode   │   │ APK Android    │   │ CLI supacode   │
 │ (VSCodium +    │   │ (pilotage à    │   │ (route, run)   │
 │  extension)    │   │  distance)     │   │                │
 └───────┬────────┘   └───────┬────────┘   └───────┬────────┘
         │                    │                    │
         └────────────────────┼────────────────────┘
                              ▼
       ┌───────────────────────────────────────┐
       │            @supacode/core             │
       │                                       │
       │  classify ─► route ─► Orchestrator    │
       │                            │          │
       │              ┌─────────────┴───┐      │
       │              ▼                 ▼      │
       │         ClaudeAgent       CodexAgent  │
       └──────────────┬─────────────────┬──────┘
                      ▼                 ▼
               CLI `claude -p`    CLI `codex exec`
               (stream-json)      (--json)
```

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

### Le flux d'événements

Chaque exécution émet d'abord un événement `route` (la décision), puis des `text`, `tool` et `log`, avec éventuellement des `error`. Elle se termine toujours par un `done` qui porte le code de sortie de l'agent. Un client (CLI, panneau de l'IDE, APK) n'a donc qu'un seul format à afficher.

## Feuille de route

1. **Fondations** *(cette étape)* : monorepo TypeScript, cœur avec classifieur, routeur et agents Claude/Codex, CLI `supacode`, CI, script VSCodium.
2. **Orchestrateur complet et panneau IDE** : branding de VSCodium, extension intégrée qui embarque le cœur, panneau de suivi des agents en direct, choix manuel de l'agent et du modèle, classification plus fine et historique des décisions.
3. **Workflows et raccourcis** : enchaînements déclaratifs du type « au commit, lancer une review + les linters », raccourcis clavier et commandes rapides pour les actions fréquentes.
4. **APK Android** : suivre et piloter le travail à distance (lancer une demande, voir le flux d'événements, valider ou arrêter une exécution) grâce à un petit serveur exposé par le cœur.
