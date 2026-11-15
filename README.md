<p align="center">
  <img src="crates/supacode-app/assets/supacode.png" width="96" alt="Logo SupaCode">
</p>

<h1 align="center">SupaCode</h1>

<p align="center"><b>L'IDE natif où tes agents IA travaillent ensemble.</b></p>

SupaCode est un IDE de bureau écrit en Rust ([GPUI](https://www.gpui.rs/)), construit autour des vrais agents de code : [Claude Code](https://docs.anthropic.com/claude-code) et [Codex](https://github.com/openai/codex). Chaque agent tourne dans son propre terminal, tu en ouvres autant que tu veux dans une grille, ils se voient et se parlent, et tu gardes la main sur tout : ce qu'ils modifient s'affiche en direct, avec son diff, et rien n'est caché.

Autour d'eux, SupaCode ajoute ce qui manque pour travailler sérieusement avec des agents : des workflows qui font office de CI/CD en local, des prompts versionnés avec git, un navigateur que les agents savent piloter, des tâches en arrière-plan à comparer, des règles d'équipe, des coûts et un journal d'audit.

Open source, licence MIT. Les contributions sont les bienvenues.

## Ce que fait SupaCode

### Plusieurs agents, côte à côte

- **La grille** : chaque « Nouvelle conversation » ajoute un terminal à la grille. Tu donnes une tâche à chacun et ils travaillent en même temps, sous tes yeux. Une tuile s'agrandit d'un clic ; les séparateurs se déplacent à la souris.
- **L'état de chaque agent** : au travail, attend ta réponse, au repos. Chaque tuile montre aussi son modèle (Haiku, Sonnet, Opus, Codex) et les messages qui l'attendent. SupaCode envoie une notification Windows quand un agent a fini ou attend une réponse, et le panneau Agents les classe par urgence.
- **Tes conversations Claude Code** : SupaCode lit `~/.claude/projects` ; un clic reprend une conversation là où tu l'avais laissée (`claude --resume`).

### Des agents qui se parlent, des sous-agents que tu pilotes

Chaque agent lancé par SupaCode reçoit son serveur MCP local :

- `agents_list` (qui travaille sur quoi), `agent_read` (l'écran d'un autre agent) et `agent_send` (un message court, remis en direct pendant sa tâche grâce aux hooks de Claude Code). Deux agents sur le même projet évitent ainsi de se marcher dessus.
- `agents_spawn` : un agent découpe une grosse tâche et crée ses sous-agents **dans la grille**. Ce ne sont pas des sous-agents invisibles : chacun est un vrai terminal que tu vois travailler, à qui tu peux écrire, que tu peux corriger (« non, garde l'ancienne route ») ou arrêter. Chacun reçoit le modèle adapté à sa tâche et renvoie son résultat à l'agent qui l'a créé.

Le serveur `supacode` et ses outils sont visibles dans l'onglet Extensions → MCP.

### Voir et relire ce que fait l'agent

- **Le code modifié en direct** : le panneau de droite suit les fichiers que l'agent change, avec leur diff, en ligne ou côte à côte.
- **Relecture** : un clic sur une ligne du diff ouvre un commentaire, envoyé à l'agent avec le fichier, la ligne et le code.
- **Points de retour** (horloge en haut de la conversation) : à chaque message envoyé, SupaCode enregistre l'état des fichiers (`refs/supacode/checkpoints/…`, sans toucher à l'index ni aux branches). « Revenir ici » restaure les fichiers ; l'état d'avant est gardé, donc annulable.
- **Taille des lots** : `max_diff_lines` dans les règles d'équipe arrête une étape d'agent qui laisse un diff trop gros pour être relu.

### Tâches en arrière-plan et comparaison

Une tâche est une demande confiée à un agent qui travaille seul, dans une copie du projet (worktree git `.supacode/worktrees/`, branche `supacode/…`). « Comparer Claude et Codex » ou « deux essais de Claude » lance plusieurs agents sur la même demande : chaque carte montre le résumé, les fichiers et le diff, et « Garder ce résultat » applique les changements au projet (sans les indexer) puis supprime les copies.

### Workflows : une CI/CD en local, pilotée par des agents

Des enchaînements d'étapes décrits dans `.supacode/workflows/*.json` : commandes, agents, validations humaines, étapes en parallèle, conditions et variables.

- **Déclencheurs** : à la main, au commit ou au push (hooks git), ou à l'enregistrement d'un fichier (`{"save": "<glob>"}`).
- **Suivi** étape par étape dans le panneau de droite, avec les cartes de validation (« Valider » / « Refuser »). Une commande en échec propose « Corriger avec l'agent ».
- **Tests d'abord** : une étape `"expect": "fail"` doit échouer (le test qui reproduit le bug). Le modèle « Correctif : tests d'abord » enchaîne test → échec attendu → correctif → tests verts → relecture.
- **Créer un workflow** : décris-le en français, un agent lit le projet et propose les étapes ; ou utilise l'éditeur visuel (blocs Commande, Agent, Validation, Parallèle, choix de l'agent et du modèle, déclencheurs).

### Des prompts versionnés

Le prompt est un livrable. La bibliothèque vit dans `.supacode/prompts/<nom>.md` (en-tête `titre`, `description`, `agent`, `variables`), versionnée par git : historique, différences entre deux versions, lancement dans une conversation avec saisie des `{{variables}}`. « Enregistrer le prompt » garde la dernière demande d'une conversation, la bibliothèque d'Anthropic fournit des exemples, et un workflow utilise un prompt avec `"prompt": "@prompts/<nom>"` (le journal note la version utilisée).

### Un navigateur que les agents savent piloter

Une vraie page web (WebView2) dans le panneau de droite. « Sélectionner » numérote des éléments, « Dessiner » permet d'entourer ou de barrer ; « Envoyer à l'agent » colle ton commentaire et une capture annotée dans la conversation. Les agents ont les outils `browser_open`, `browser_snapshot`, `browser_click`, `browser_type`, `browser_screenshot`, `browser_console`… pour tester une fonctionnalité sous tes yeux.

### Le bon modèle pour chaque demande

Un orchestrateur classe chaque demande (question, petite correction, refacto, fonctionnalité, revue) et estime sa taille, puis choisit l'agent et le modèle : une typo part chez Codex, une question chez Haiku, une refonte chez Opus. Il sert aux workflows, aux tâches et aux sous-agents, et ses règles se règlent par projet (voir [Configuration](#configuration)).

### Extensions

Les skills de l'utilisateur (`~/.claude/skills`) et du projet (`.claude/skills`), les serveurs MCP et les plugins de Claude Code, avec un interrupteur chacun. Désactiver passe par `claude --settings` : ta configuration de Claude Code n'est pas réécrite. La bibliothèque cherche sur SkillsMP et dans les skills officiels d'Anthropic, ou installe n'importe quel dossier GitHub qui contient un `SKILL.md`.

### Pour une équipe

Sans serveur, tout est dans le dépôt :

- **Règles d'équipe** (`.supacode/policy.json`, versionné) : agents et modèles permis, commandes bloquées (`git push --force*`, `rm -rf *`…), fichiers protégés (`.env`, `secrets/**`…), budgets par jour, par mois et par workflow, navigateur permis ou non.
- **Coûts estimés** au prix public de l'API, total du jour dans la barre d'état ; un budget dépassé bloque les étapes d'agent.
- **Journal d'audit** : conversations, commandes, agents (modèle, outils, coût), validations, blocages, avec l'utilisateur git ; exportable en CSV.

### Et aussi

**Ctrl+K** (ou Ctrl+P) ouvre une palette pour retrouver une conversation, un fichier, un projet ou une commande.

## Installer et lancer

Prérequis :

- [Rust](https://rustup.rs/) 1.85 ou plus (édition 2024). Sous Windows, les Build Tools de Visual Studio (MSVC). Sous Linux, les bibliothèques de GPUI (`libxkbcommon-dev`, `libwayland-dev`, `libvulkan-dev`…).
- La CLI [`claude`](https://docs.anthropic.com/claude-code) installée et connectée ; [`codex`](https://github.com/openai/codex) en option.

```bash
git clone https://github.com/Charoxy/SupaCode
cd SupaCode
cargo run -p supacode-app --release
```

L'exécutable est `target/release/supacode.exe` (sous Windows, il porte l'icône de SupaCode : épingle-le ou crée un raccourci vers lui).

## Configuration

Place un fichier `.supacode/config.json` à la racine de ton projet pour régler l'orchestrateur. Il est fusionné avec la configuration par défaut : tu peux n'indiquer que ce qui change.

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

Le format des workflows est décrit dans [docs/architecture.md](docs/architecture.md#workflows).

## La CLI `supacode`

Les mêmes briques existent en ligne de commande (TypeScript, Node 20 ou plus) :

```bash
npm install
npm run build
node packages/cli/dist/index.js route "corrige la typo dans le README"   # quel agent serait choisi
node packages/cli/dist/index.js run "ajoute un test pour la fonction slugify"
node packages/cli/dist/index.js workflow run verifs --yes
node packages/cli/dist/index.js hooks install                              # workflows au commit et au push
```

`npm link -w @supacode/cli` installe la commande `supacode` globalement.

## Structure

| Chemin | Rôle |
| --- | --- |
| `crates/supacode-app` | L'IDE natif (GPUI) : grille de terminaux, panneau de code et de diff, workflows, tâches, prompts, navigateur, extensions, équipe, serveur MCP des agents |
| `crates/supacode-core` | Cœur en Rust : conversations Claude Code, classifieur et routeur, agents Claude et Codex, moteur de workflows, règles d'équipe, coûts, audit |
| `packages/core` | `@supacode/core` : le même cœur en TypeScript |
| `packages/cli` | `@supacode/cli` : commande `supacode` (`route`, `run`, `workflow`, `hooks`) |
| `docs/` | [Architecture et feuille de route](docs/architecture.md) |

## Contribuer

Les issues et les pull requests sont les bienvenues. Avant d'en ouvrir une :

```bash
cargo test --workspace
npm test
```

L'intégration continue (GitHub Actions) fait le build et les tests Rust et TypeScript à chaque push. Le code, les commentaires et l'interface sont en français.

## Licence

[MIT](LICENSE) © Charoxy
