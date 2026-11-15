---
name: roblox-rojo
description: Synchroniser et compiler un projet Roblox avec Rojo. À utiliser pour créer ou réparer un projet Rojo (default.project.json), lancer rojo serve pour Studio, produire une place .rbxlx avec rojo build, installer les paquets Wally, ou passer Selene et StyLua sur le code Luau.
---

# Projet Roblox avec Rojo (synchro et compilation de la place)

Rojo fait le lien entre les fichiers du dépôt (Luau, JSON, modèles) et Roblox Studio.
Le fichier `*.project.json` (en général `default.project.json`) décrit où chaque dossier
arrive dans l'arbre du jeu.

## 1. Repérer la chaîne d'outils

Regarde la racine du projet avant toute commande :

- `default.project.json` (ou un autre `*.project.json`) : projet Rojo.
- `rokit.toml`, `aftman.toml` ou `foreman.toml` : versions des outils. Lance `rokit install`
  (ou `aftman install`) si `rojo` n'est pas trouvé.
- `wally.toml` : paquets. `wally install` crée `Packages/` (et `ServerPackages/`).
- `selene.toml` : linter. `stylua.toml` : formateur.

Vérifie la version : `rojo --version`. Sous Windows, si `rojo` n'est pas dans le PATH, il
est souvent dans `%USERPROFILE%\.rokit\bin\` ou `%USERPROFILE%\.aftman\bin\`.

## 2. Structure type d'un projet

```json
{
  "name": "MonJeu",
  "tree": {
    "$className": "DataModel",
    "ReplicatedStorage": {
      "Shared": { "$path": "src/shared" },
      "Packages": { "$path": "Packages" }
    },
    "ServerScriptService": { "Server": { "$path": "src/server" } },
    "StarterPlayer": {
      "StarterPlayerScripts": { "Client": { "$path": "src/client" } }
    }
  }
}
```

Conventions de nommage des fichiers Luau :

| Fichier | Devient |
| --- | --- |
| `Nom.server.luau` | `Script` (serveur) |
| `Nom.client.luau` | `LocalScript` |
| `Nom.luau` | `ModuleScript` |
| `init.luau` dans un dossier | le dossier devient le script lui-même |
| `Nom.model.json`, `.rbxm`, `.rbxmx` | instances ou modèles |

Après un changement de `*.project.json`, relance `rojo serve` : il ne relit pas toujours
l'arbre à chaud.

## 3. Synchroniser avec Studio

1. `rojo serve` (port 34872 par défaut, ou `servePort` du projet). SupaCode peut le lancer
   depuis l'onglet Roblox.
2. Dans Studio, plugin Rojo, bouton **Connect**.
3. Les fichiers du disque font foi : une modification faite dans Studio sur un script géré
   par Rojo est écrasée à la prochaine synchro. Écris le code dans les fichiers.

## 4. Produire la place

```sh
rojo build default.project.json -o build/MonJeu.rbxlx
```

- `.rbxlx` (XML, lisible) ou `.rbxl` (binaire, plus compact).
- Ouvre-la dans Studio pour tester (`start build\MonJeu.rbxlx` sous Windows).
- Pour un modèle ou un paquet seul : un projet dont la racine n'est pas `DataModel`, puis
  `rojo build modele.project.json -o modele.rbxm`.

## 5. Vérifier avant de rendre la main

Lance ce qui existe dans le projet, dans cet ordre :

```sh
wally install                   # si wally.toml
rojo sourcemap default.project.json -o sourcemap.json   # pour luau-lsp et wally-package-types
selene src                      # si selene.toml
stylua --check src              # si stylua.toml (stylua src pour corriger)
rojo build default.project.json -o build/check.rbxlx
```

Un build qui passe ne prouve pas que le jeu marche : dis à l'utilisateur ce qui reste à
tester dans Studio (Play, deux joueurs en local, appareil mobile…).

## Erreurs fréquentes

- **« Couldn't find path »** : un `$path` vise un dossier absent. Crée-le ou corrige le JSON.
- **Packages introuvables au runtime** : `wally install` n'a pas été lancé, ou `Packages`
  n'est pas monté dans `ReplicatedStorage`.
- **Script qui ne s'exécute pas** : mauvaise extension (`.server` / `.client`) ou script
  serveur placé dans `ReplicatedStorage`.
- **Port occupé** : un autre `rojo serve` tourne déjà (souvent pour un autre projet).

## Outils Studio

Si les outils MCP de Roblox Studio sont disponibles (`mcp__Roblox_Studio__*`), sers-t'en
pour vérifier le résultat dans Studio : lire la console, inspecter l'arbre, lancer un test.
