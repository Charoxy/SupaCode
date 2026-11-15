---
name: roblox-blender-model
description: Créer un modèle 3D pour Roblox avec Blender en ligne de commande (sans interface) - un script Python bpy génère le maillage, le vérifie (triangles, taille, matériaux), l'exporte en .glb ou .fbx et rend une image d'aperçu. À utiliser quand l'utilisateur veut un objet, un décor, une arme, un bâtiment ou tout asset 3D pour son jeu Roblox.
---

# Modèles 3D pour Roblox avec Blender sans interface

Le principe : écrire un script Python (`bpy`) qui construit le modèle, le lancer avec
Blender en arrière-plan, regarder l'aperçu rendu, corriger, recommencer. Rien n'est fait à
la souris : le script est la source du modèle, on peut le relancer et le modifier.

## 1. Trouver Blender

```sh
blender --version
```

Sous Windows, s'il n'est pas dans le PATH, il est en général dans
`C:\Program Files\Blender Foundation\Blender <version>\blender.exe` (prends la version la
plus récente). Note le chemin complet et réutilise-le. Les exemples ci-dessous écrivent
`blender` pour faire court.

## 2. Partir du gabarit

Ce skill fournit deux scripts dans son dossier `scripts/` :

- `model_template.py` : scène vide, unités en studs, fonction `build()` à remplir, puis
  nettoyage, contrôle des limites de Roblox et export.
- `preview.py` : rend une image PNG du fichier exporté (vue 3/4, moteur Workbench, rapide
  et sans GPU).

Copie le gabarit dans le projet (par exemple `assets/models/<nom>/<nom>.py`), remplis
`build()`, puis :

```sh
blender -b --factory-startup -P assets/models/caisse/caisse.py -- --out assets/models/caisse/caisse.glb
blender -b --factory-startup -P <dossier du skill>/scripts/preview.py -- assets/models/caisse/caisse.glb assets/models/caisse/caisse.png
```

`-b` : sans interface. `--factory-startup` : sans les réglages ni add-ons de l'utilisateur
(résultat reproductible). Tout ce qui suit `--` est pour le script.

**Regarde toujours l'image d'aperçu** (outil de lecture d'image) avant de dire que le modèle
est prêt. Corrige proportions, orientation et couleurs, puis relance.

## 3. Règles pour que l'import dans Roblox se passe bien

- **Échelle** : le gabarit travaille en studs (1 unité Blender = 1 stud ; un personnage
  Roblox mesure environ 5 studs, une porte 4 × 7). À l'import, choisis l'unité « Stud »
  dans l'importeur 3D de Studio si l'option est proposée, sinon vérifie et ajuste la taille
  de la MeshPart. Pour rappel, 1 stud vaut environ 0,28 m.
- **Orientation** : Roblox a l'axe Y vers le haut et regarde vers -Z. Les exportateurs
  glTF et FBX convertissent depuis le Z vers le haut de Blender : modélise normalement
  (Z en haut, l'avant vers -Y dans Blender).
- **Triangles** : reste sous 20 000 triangles par maillage (limite de Roblox pour une
  MeshPart, à confirmer dans le message d'erreur de l'importeur si elle a changé). Vise bien
  moins pour un objet répété : 500 à 3 000 pour un accessoire, quelques milliers pour un
  bâtiment. Le gabarit affiche le total et refuse l'export au-delà de la limite.
- **Origine** : place l'origine au pied de l'objet (z = 0 au sol), centrée. Le gabarit le fait.
- **Transformations appliquées** : rotation et échelle à 1 avant l'export (fait par le gabarit).
- **Matériaux** : une couleur de base par matériau suffit souvent, Roblox la reprend en
  SurfaceAppearance ou en couleur. Textures en PNG, 1024 × 1024 au plus (Roblox réduit
  au-delà), chemins relatifs, ou bien intégrées dans le .glb.
- **Plusieurs pièces** : un objet Blender = une MeshPart. Fusionne ce qui doit rester
  solidaire, sépare ce qui doit bouger (porte, roue).
- **Collisions** : une forme simple est meilleure pour le jeu. Indique à l'utilisateur de
  régler `CollisionFidelity` (Box ou Hull) sur les objets décoratifs.

## 4. Format d'export

- `.glb` (recommandé) : un seul fichier, textures intégrées, bien pris en charge par
  l'importeur 3D de Studio.
- `.fbx` : à préférer pour un objet animé avec squelette (rig), ou si l'utilisateur le demande.

Le gabarit choisit le format d'après l'extension de `--out`.

## 5. Construire en Python : rappels bpy

- Primitives : `bpy.ops.mesh.primitive_cube_add(size=1, location=(0, 0, 0.5))`,
  `primitive_cylinder_add(vertices=16, radius=…, depth=…)`, `primitive_uv_sphere_add(segments=16, ring_count=8)`.
  Garde peu de segments : c'est du jeu vidéo, pas du rendu.
- Formes sur mesure : `bmesh` (créer des sommets et des faces, extruder), puis
  `bm.to_mesh(mesh)`.
- Modificateurs utiles : `BEVEL` (arêtes adoucies, 1 ou 2 segments), `MIRROR`, `ARRAY`,
  `SOLIDIFY`, `DECIMATE` (réduire les triangles). Le gabarit les applique à l'export.
- Nœuds de matériau : cherche les nœuds par **type**, jamais par nom (les noms sont
  traduits) : `next(n for n in mat.node_tree.nodes if n.type == "BSDF_PRINCIPLED")`.
- Ne te fie pas aux identifiants d'énumérations d'une version précise de Blender : lis-les
  si besoin (`bpy.types.<Type>.bl_rna.properties[...]`), entoure les changements de moteur
  de rendu d'un `try/except TypeError`.

## 6. Rendre la main

Donne à l'utilisateur :

1. le chemin du fichier exporté et de l'aperçu (montre l'image) ;
2. le nombre de triangles et la taille en studs (affichés par le gabarit) ;
3. comment l'importer : Studio, onglet **Home** ou **Avatar**, **Import 3D**, choisir le
   fichier, vérifier l'échelle dans l'aperçu, puis **Import** ;
4. le script source, pour pouvoir régénérer une variante.

Si les outils MCP de Roblox Studio sont disponibles (`mcp__Roblox_Studio__*`), propose de
placer le modèle dans la place ouverte ou d'inspecter le résultat dans Studio.
