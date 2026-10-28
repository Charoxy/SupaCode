# SupaCode

**Un IDE pour coder avec l'IA, qui choisit pour toi le bon agent et le bon modèle.**

Tu décris ce que tu veux. L'orchestrateur de SupaCode classe la demande (question, petite correction, refacto, nouvelle fonctionnalité, review) et estime sa taille. Il l'envoie ensuite à l'agent le plus adapté : [Claude Code](https://docs.anthropic.com/claude-code) (Haiku, Sonnet ou Opus) ou [Codex](https://github.com/openai/codex). Une petite typo part chez Codex, une refonte d'architecture chez Claude Opus. Plus tard, une application Android permettra de piloter le travail à distance.

## Ce qui marche aujourd'hui

- **L'IDE** (`crates/`) : une application native écrite en Rust (GPUI), organisée en trois colonnes. À gauche, tes projets et leurs conversations Claude Code, puis tes workflows. Au centre, la conversation elle-même, c'est-à-dire la vraie CLI `claude` dans un terminal intégré. À droite, l'aperçu du code que Claude modifie (avec son diff), mis à jour en direct, ou le suivi d'un workflow en cours avec ses cartes de validation.
- **Les workflows** : des enchaînements d'étapes (commandes, agents, validations, étapes en parallèle) avec conditions et variables, décrits dans `.supacode/workflows/*.json`. Ils se déclenchent à la main, au commit ou au push (hooks git), ou à l'enregistrement d'un fichier.
- `@supacode/core` : classifieur de demandes (mots-clés français et anglais), routeur configurable, pilotage des CLI `claude` et `codex` avec un flux d'événements commun, et moteur de workflows.
- `@supacode/cli` : la commande `supacode` (`route`, `run`, `workflow`, `hooks`).
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

### Workflows

```bash
supacode workflow list                 # workflows de .supacode/workflows
supacode workflow run verifs --yes     # lance un workflow (--yes valide les étapes « approve »)
supacode hooks install                 # lance les workflows « commit » et « push » via les hooks git
```

Le format est décrit dans [docs/architecture.md](docs/architecture.md#workflows).

### L'IDE

Prérequis : Rust 1.85 ou plus (édition 2024). Sous Windows, les Build Tools Visual Studio (MSVC). Sous Linux, les bibliothèques de GPUI (`libxkbcommon-dev`, `libwayland-dev`, `libvulkan-dev`, etc.).

```bash
cargo run -p supacode-app --release
cargo test --workspace
```

L'IDE lit les conversations de Claude Code dans `~/.claude/projects` ; un clic sur une conversation la reprend (`claude --resume`).

- **Ctrl+K** (ou Ctrl+P) : palette pour retrouver une conversation, un fichier, un projet ou une commande.
- **Workflows** : ▶ pour lancer, suivi étape par étape à droite avec les validations ; les workflows `{"save": "<glob>"}` se lancent seuls à l'enregistrement d'un fichier correspondant.
- **Nouveau workflow** (bouton + de la section Workflows) : décris-le en français, Claude lit le projet et propose les étapes ; tu vérifies l'aperçu puis tu le crées, ou tu l'ouvres dans l'éditeur.
- **Éditeur visuel** (crayon sur un workflow, ou Ctrl+K) : blocs Commande, Agent, Validation et Parallèle, conditions, choix de l'agent et du modèle, déclencheurs ; « Tester » enregistre et lance le workflow.
- **Navigateur** (globe dans la barre de gauche, ou Ctrl+K) : une vraie page web (WebView2) dans le panneau de droite. « Sélectionner » numérote des éléments de la page, « Dessiner » permet d'entourer ou de barrer ; « Envoyer à l'agent » colle ton commentaire, les éléments choisis et une capture annotée (`.supacode/captures/`, ignoré par git) dans la conversation ouverte.
- **Prompts** (bulle dans la barre de gauche) : le prompt est un livrable. La bibliothèque vit dans `.supacode/prompts/<nom>.md` (en-tête `titre`, `description`, `agent`, `variables`), versionnée par git : historique, comparaison entre deux versions, lancement dans une conversation avec saisie des `{{variables}}`. « Enregistrer le prompt » garde la dernière demande d'une conversation. Un workflow utilise un prompt de la bibliothèque avec `"prompt": "@prompts/<nom>"` ; le journal note la version utilisée.
- **Tests d'abord** : une étape `"expect": "fail"` doit échouer (un test qui reproduit le bug) ; si elle passe, le workflow s'arrête. Le modèle « Correctif : tests d'abord » (section Modèles des workflows) enchaîne test → échec attendu → correctif → tests verts → relecture, avec la commande de test du projet. `"ask"` pose une question au lancement, dont la réponse devient `{{saisie}}`.
- **Taille des lots** : `max_diff_lines` dans les règles d'équipe arrête une étape d'agent qui laisse un diff trop gros pour être relu ; le panneau de code affiche le total face à la limite.
- **Extensions** (pièce de puzzle) : les skills de l'utilisateur (`~/.claude/skills`) et du projet (`.claude/skills`), et les serveurs MCP, avec un interrupteur chacun. Désactiver passe par `claude --settings` (`skillOverrides`, interdiction `mcp__<serveur>`, `disabledMcpjsonServers`) : la configuration de Claude Code n'est pas réécrite. La bibliothèque cherche sur SkillsMP (clé API facultative) et dans les skills officiels d'Anthropic, ou installe n'importe quel dossier GitHub qui contient un `SKILL.md`, pour soi ou pour le projet.
- **Équipe** (bouclier dans la barre de gauche) : règles d'équipe, coûts et journal d'audit, sans serveur.
  - **Règles d'équipe** : `.supacode/policy.json`, versionné avec le code, donc les mêmes pour tout le monde. Agents et modèles permis, commandes bloquées (`git push --force*`, `rm -rf *`…), fichiers protégés (`.env`, `secrets/**`…), budgets par jour, par mois et par workflow, navigateur permis ou non. Les commandes des workflows sont vérifiées avant de tourner ; Claude reçoit les interdictions par `--settings` et les applique avant chaque outil ; Codex est vérifié après coup.
  - **Coûts estimés** : au prix public de l'API Claude, à partir des transcripts de Claude Code et du coût annoncé par les étapes d'agent ; total du jour dans la barre d'état. Un budget dépassé bloque les étapes d'agent des workflows.
  - **Journal d'audit** : conversations, commandes, agents (modèle, outils, coût), validations, blocages, navigateur, avec l'utilisateur git ; `.supacode/journal/` (ignoré par git), exportable en CSV.
- **Les agents pilotent le navigateur** : chaque `claude` lancé par l'IDE reçoit le serveur MCP local de SupaCode (`browser_open`, `browser_snapshot`, `browser_click`, `browser_type`, `browser_screenshot`, `browser_console`…) pour tester une fonctionnalité sous tes yeux.

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
| `packages/core` | `@supacode/core` : types, classifieur, configuration, routeur, agents Claude/Codex, orchestrateur, moteur de workflows |
| `packages/cli` | `@supacode/cli` : commande `supacode` (`route`, `run`, `workflow`, `hooks`) |
| `crates/supacode-core` | Cœur en Rust : conversations Claude Code, classifieur et routeur, agents Claude/Codex, moteur de workflows |
| `crates/supacode-app` | L'IDE natif (GPUI) : fenêtre, terminal intégré, aperçu du code, suivi des workflows |
| `docs/` | Documentation ([architecture et feuille de route](docs/architecture.md)) |
| `.github/workflows` | Intégration continue |

## Licence

[MIT](LICENSE) © Charoxy
