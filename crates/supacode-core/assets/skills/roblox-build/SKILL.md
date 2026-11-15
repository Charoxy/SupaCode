---
name: roblox-build
description: Construire de beaux décors dans Roblox Studio - bâtiments, maps, intérieurs, villages, îles, obbys, props. Méthode de level design (échelle en studs, maquette puis détails, palette et matériaux, lumière, végétation), bibliothèque Luau pour placer murs, ouvertures, toits et escaliers sans erreur, et vérification par captures d'écran. À utiliser dès qu'il faut construire ou embellir quelque chose dans une place Roblox.
---

# Construire dans Roblox Studio

Un décor réussi ne vient pas d'un seul gros script : il vient de **passes successives**,
chacune vérifiée à l'image. Ne livre jamais une construction que tu n'as pas regardée.

## 0. Outils

- Si les outils MCP de Roblox Studio sont là (`mcp__Roblox_Studio__*`), c'est le chemin
  principal : `get_studio_state` (place ouverte, mode), `execute_luau` (construire),
  `screen_capture` (regarder), `search_game_tree` / `inspect_instance` (vérifier l'arbre),
  `search_asset` / `insert_asset` (props du Creator Store), `generate_mesh`,
  `generate_material`, `generate_texture` (assets générés), `start_stop_play` et
  `character_navigation` (se promener dedans). Lis leurs descriptions avant de t'en servir.
- Sans ces outils : écris un script de construction (ModuleScript ou commande à coller
  dans la barre de commande de Studio) et demande à l'utilisateur une capture d'écran.
- La bibliothèque `scripts/Builder.luau` de ce skill fait la géométrie délicate (murs avec
  portes et fenêtres, toits à deux pans, escaliers, rangées). Charge-la au début de chaque
  `execute_luau` (colle son contenu, puis utilise `Builder`).

## 1. Avant de poser une pièce : le plan

Écris en quelques lignes, et montre-le à l'utilisateur si la demande est vague :

- **Ce que c'est et à quoi ça sert dans le jeu** : où le joueur arrive, où il va, ce qu'il
  doit remarquer en premier (point focal).
- **Style** : réaliste, low-poly, cartoon, médiéval, sci-fi… et la palette (3 à 5 couleurs
  principales et 1 couleur d'accent).
- **Emprise et cotes en studs** (tableau ci-dessous), position dans la place (ne construis
  pas par-dessus l'existant : regarde d'abord avec `search_game_tree` et une capture).

### Échelle de référence (un personnage R15 mesure environ 5 studs)

| Élément | Cote habituelle |
| --- | --- |
| Porte | 4 à 5 × 7 à 8 de haut |
| Fenêtre | 3 à 4 × 4, allège à 3 du sol |
| Hauteur d'étage | 12 à 14 (sol à sol) |
| Épaisseur de mur | 1 (intérieur) à 1,5 (extérieur) |
| Marche | 1 de haut, 1,5 à 2 de profondeur |
| Garde-corps | 3 de haut |
| Couloir | 8 à 10 de large (les joueurs se croisent, caméra à l'aise) |
| Route | 20 à 30 de large ; trottoir 6 à 8 |
| Arbre | 15 à 35 de haut |
| Pièce intérieure | 20 × 20 au minimum (la caméra doit tenir) |

Les jeux Roblox sont **plus grands que nature** : élargis les intérieurs et les passages
de 20 à 50 % par rapport au réel, sinon la caméra se coince.

## 2. Passe 1 : la maquette (blockout)

- Uniquement des volumes simples, couleurs neutres, matériau `SmoothPlastic`.
- Pose un personnage de référence (un `Rig` R15 ou un bloc de 2 × 5 × 1) près de l'entrée.
- Vérifie : proportions, circulation (le joueur passe partout, sauts faisables : 7 studs de
  haut au plus avec la gravité par défaut, écarts de 10 à 12 studs au plus), lisibilité du
  chemin principal.
- Capture d'écran de 3 points de vue (voir §6). Corrige avant d'aller plus loin.

## 3. Passe 2 : formes et détails

Ce qui fait la différence entre « des cubes » et « un bâtiment » :

- **Profondeur** : rien n'est plat. Encadrements de portes et fenêtres en saillie de 0,2 à
  0,5 stud, soubassement plus large en bas des murs, corniche sous le toit, poutres
  apparentes, rebords de fenêtre.
- **Silhouette** : casse les lignes droites. Toits avec débord (1 à 2 studs), cheminée,
  lucarne, avancée, balcon, hauteurs différentes entre les bâtiments voisins.
- **Biseaux** : `WedgePart` et `CornerWedgePart` pour les pentes, chanfreins et toits ;
  cylindres pour poteaux et rondins.
- **Hiérarchie des détails** : grandes formes → éléments moyens (fenêtres, escaliers,
  clôtures) → petits accessoires (caisses, lampes, pots, panneaux). Regroupe les petits
  accessoires en grappes (3 à 5 objets) plutôt que de les éparpiller.
- **Répétition maîtrisée** : un module (fenêtre, travée, lampadaire) répété à pas régulier
  (`Builder.array`), avec de petites variations (rotation de quelques degrés, teinte
  légèrement différente) pour éviter l'effet copier-coller.
- **Asymétrie volontaire** dans les décors naturels ; symétrie pour l'officiel (château,
  temple, mairie).

## 4. Passe 3 : matériaux et couleurs

- Matériaux Roblox adaptés : `WoodPlanks`, `Wood`, `Brick`, `Cobblestone`, `Slate`,
  `Concrete`, `Plaster` (si disponible), `Metal`, `DiamondPlate`, `Fabric`, `Grass`,
  `Sand`, `Rock`, `Marble`, `Glass` (avec `Transparency` 0,3 à 0,6) ; `SmoothPlastic`
  pour le style cartoon ou low-poly. Vérifie le nom dans `Enum.Material` en cas de doute.
- **Pas de couleurs pures** (255, 0, 0) : des tons légèrement désaturés. Les murs clairs, le
  bois et la pierre moyens, les accents saturés réservés au point focal.
- **Variation de teinte** : deux pièces voisines du même matériau n'ont pas exactement la
  même couleur (± 3 à 8 % de luminosité). `Builder.vary(color)` le fait.
- Même style dans toute la zone : pas de briques réalistes à côté de blocs cartoon.

## 5. Passe 4 : lumière, ambiance, végétation, terrain

- `Lighting` : `Technology` en `Future` si le projet le permet (sinon `ShadowMap`),
  `ClockTime` autour de 14 (jour) ou 17,5 (golden hour), `Ambient` et `OutdoorAmbient`
  pas trop sombres.
- Ajoute (ou règle) `Atmosphere` (`Density` 0,3 environ, `Haze` faible), un
  `ColorCorrectionEffect` léger (saturation +0,05 à +0,15, contraste +0,05) et un
  `BloomEffect` discret. Ne change l'ambiance d'une place existante qu'avec l'accord de
  l'utilisateur.
- Lumières locales : `PointLight` ou `SpotLight` dans les lampes, couleur chaude
  (255, 200, 140), `Range` 12 à 20, `Brightness` 1 à 2. Pas plus de quelques dizaines dans
  la scène.
- Terrain naturel : `workspace.Terrain:FillBlock`, `FillBall`, `FillCylinder`,
  `FillWedge` avec `Enum.Material.Grass`, `Rock`, `Sand`, `Water`… Mélange les matériaux
  (herbe sur le dessus, roche sur les pentes). Évite les bords nets en superposant des boules.
- Végétation : arbres et buissons en grappes, de tailles variées, jamais alignés au cordeau.
  Le Creator Store (`search_asset`) a de bons arbres ; vérifie le modèle inséré (pas de
  scripts inattendus, nombre de pièces raisonnable).

## 6. Regarder son travail (obligatoire à chaque passe)

1. Place la caméra de Studio avec `execute_luau` :
   `workspace.CurrentCamera.CFrame = CFrame.lookAt(position, cible)`.
2. `screen_capture`, puis regarde vraiment l'image.
3. Au moins trois vues : hauteur d'yeux du joueur (y ≈ cible + 5) depuis le chemin
   d'arrivée, vue de trois quarts en hauteur, et une vue rapprochée d'un détail.
4. Critique avec cette liste, corrige, recommence :
   - Les proportions sont-elles bonnes à côté du personnage de référence ?
   - Y a-t-il des surfaces plates et vides sur plus de 10 × 10 studs ?
   - Des pièces qui se chevauchent en scintillant (z-fighting) ou qui flottent ?
   - Le point focal se voit-il en premier ? Le chemin est-il évident ?
   - La palette est-elle cohérente ? Trop de couleurs ?
   - Le joueur peut-il passer partout où il doit passer (et pas ailleurs) ?

Si `character_navigation` et `start_stop_play` sont disponibles, fais un tour en mode Play
pour vérifier collisions et circulation, puis repasse en mode édition.

## 7. Propreté et performances

- Tout dans un `Model` nommé (par exemple `Workspace.Village.Maison1`), avec `PrimaryPart`,
  sous-dossiers par catégorie (`Structure`, `Deco`, `Lumieres`). Noms explicites.
- Toutes les pièces `Anchored = true`.
- Petits décors : `CanCollide = false`, `CanTouch = false`, `CanQuery = false`, et
  `CastShadow = false` pour les très petits (`Builder` a une option `deco = true`).
- Évite le z-fighting : deux faces ne doivent jamais coïncider. Décale de 0,01 à 0,05 ou
  fais se croiser franchement les volumes.
- Coordonnées arrondies (grille de 0,05 stud) et rotations à 15°, 45° ou 90° près, sauf
  variation volontaire.
- Budget : quelques centaines de pièces pour un bâtiment détaillé, quelques milliers pour
  une zone. Au-delà, utilise des maillages (skill `roblox-blender-model`, `generate_mesh`)
  ou des unions, avec parcimonie.
- Une construction par appel `execute_luau` raisonnable : découpe les grosses zones
  (une maison, puis la suivante) pour pouvoir vérifier et annuler.
- Annulation : enregistre un point d'historique (`ChangeHistoryService`) avant et après
  une construction quand c'est possible, pour que Ctrl+Z la défasse d'un coup.

## 8. Rendre la main

Montre les captures finales, dis où se trouve le modèle dans l'arbre, le nombre de pièces,
ce qui reste à faire (intérieurs, éclairage de nuit, scripts de portes…), et propose deux
ou trois améliorations concrètes.
