---
name: roblox-luau
description: Écrire du code Luau solide pour un jeu Roblox - séparation serveur et client, RemoteEvents validés côté serveur, sauvegarde des données avec DataStoreService, performances et Luau typé. À utiliser pour toute fonctionnalité de gameplay, d'interface ou de données dans un jeu Roblox.
---

# Luau pour Roblox

## Où vit le code

| Emplacement | Exécuté par | Usage |
| --- | --- | --- |
| `ServerScriptService` | serveur | logique de jeu, données, autorité |
| `ServerStorage` | serveur (invisible aux clients) | modèles et modules réservés au serveur |
| `ReplicatedStorage` | les deux | modules partagés, remotes, assets |
| `StarterPlayerScripts` | client | contrôles, caméra, effets |
| `StarterGui` | client | interface |

Le serveur fait autorité : tout ce qui touche à l'argent, l'inventaire, les dégâts, la
progression se décide côté serveur. Le client affiche et demande.

## Remotes : ne jamais faire confiance au client

```luau
-- Serveur
local Remotes = ReplicatedStorage.Remotes
Remotes.BuyItem.OnServerEvent:Connect(function(player: Player, itemId: unknown)
	if typeof(itemId) ~= "string" then return end          -- type
	local item = Items[itemId]
	if not item then return end                              -- existence
	if not cooldown(player, "BuyItem", 0.25) then return end -- fréquence
	local data = PlayerData.get(player)
	if not data or data.coins < item.price then return end   -- règles du jeu
	data.coins -= item.price
	table.insert(data.inventory, itemId)
end)
```

- Vérifie le type, l'existence, la distance ou la portée, et la fréquence de chaque appel.
- N'accepte jamais un prix, des dégâts ou une position « de confiance » envoyés par le client.
- `RemoteFunction` du serveur vers le client : à éviter (un client peut ne jamais répondre).
- Regroupe les remotes dans un dossier `Remotes` créé par le serveur ou par Rojo.

## Données des joueurs

- `DataStoreService` : toujours dans un `pcall`, avec nouvelles tentatives espacées.
- Préfère `UpdateAsync` à `SetAsync` pour ne pas écraser une écriture concurrente.
- Charge à `PlayerAdded`, sauvegarde à `PlayerRemoving`, périodiquement (toutes les quelques
  minutes) et dans `game:BindToClose` (avec un délai court en Studio).
- Garde une version de schéma dans les données et une fonction de migration.
- Si le projet utilise déjà ProfileStore, ProfileService ou DataStore2 (voir `wally.toml`),
  passe par lui plutôt que d'appeler DataStoreService directement.

## Performances

- Pas de boucle `while true do wait() end` : utilise `RunService.Heartbeat` (serveur),
  `RenderStepped` / `PreRender` (client, caméra) ou `task.wait(n)`.
- `task.spawn`, `task.defer`, `task.delay` plutôt que `spawn`, `delay`, `wait` (dépréciés).
- Déconnecte les connexions (`:Disconnect()`) quand un objet disparaît ; nettoie avec un
  « Trove » ou « Maid » si le projet en a un.
- Mets en cache les services et les références (`local Players = game:GetService("Players")`).
- Beaucoup d'objets identiques : réutilise-les (pool) au lieu de `Instance.new` / `:Destroy()`
  en boucle. Effets purement visuels : côté client.
- `CollectionService` (tags) pour donner un comportement à tous les objets d'un type.

## Luau typé

- `--!strict` en tête des nouveaux modules quand le projet le permet.
- Types exportés dans les modules partagés : `export type ItemId = string`.
- Annoter les paramètres des remotes en `unknown`, puis affiner avec `typeof`.

## Style

- Respecte le style du projet (`stylua.toml`, `selene.toml`) et lance-les après tes changements.
- Modules courts, une responsabilité chacun ; pas de variables globales.
- Messages d'erreur clairs pour le développeur (`warn`, `error`) ; jamais d'erreur affichée
  au joueur sans traduction en message de jeu.

## Tester

- Dans Studio : **Play** (un joueur), **Test > Clients and Servers** (plusieurs joueurs)
  pour la réplication et les remotes.
- Si les outils MCP de Roblox Studio sont disponibles (`mcp__Roblox_Studio__*`), lance le
  test, lis la console et inspecte l'arbre toi-même avant de rendre la main.
- Dis à l'utilisateur ce que tu n'as pas pu tester (mobile, manette, plusieurs serveurs).
