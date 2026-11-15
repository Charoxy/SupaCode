"""Gabarit de modèle 3D pour Roblox, à lancer avec Blender sans interface.

    blender -b --factory-startup -P mon_modele.py -- --out mon_modele.glb

Remplis `build()` : c'est la seule partie à écrire. Le reste nettoie la scène, met
l'origine au pied de l'objet, applique les transformations, vérifie le nombre de
triangles et exporte en .glb ou .fbx selon l'extension de --out.

Unités : 1 unité Blender = 1 stud Roblox.
"""

import argparse
import sys

import bpy

# Limite de triangles d'une MeshPart dans Roblox.
MAX_TRIANGLES = 20_000


# ---------------------------------------------------------------------------
# À écrire : construction du modèle
# ---------------------------------------------------------------------------


def build():
    """Crée les objets du modèle. Exemple : une caisse en bois de 3 studs."""
    crate = add_box("Caisse", size=(3, 3, 3), location=(0, 0, 1.5))
    bevel(crate, width=0.12, segments=2)
    set_material(crate, material("Bois", (0.55, 0.36, 0.2)))

    # Planches en relief sur une face.
    for i, z in enumerate((0.6, 1.5, 2.4)):
        plank = add_box(f"Planche{i}", size=(3.1, 0.12, 0.45), location=(0, -1.55, z))
        set_material(plank, material("BoisFonce", (0.38, 0.24, 0.13)))


# ---------------------------------------------------------------------------
# Aides
# ---------------------------------------------------------------------------


def add_box(name, size=(1, 1, 1), location=(0, 0, 0), rotation=(0, 0, 0)):
    bpy.ops.mesh.primitive_cube_add(size=1, location=location, rotation=rotation)
    obj = bpy.context.active_object
    obj.name = name
    obj.scale = size
    return obj


def add_cylinder(name, radius=0.5, depth=1, vertices=16, location=(0, 0, 0), rotation=(0, 0, 0)):
    bpy.ops.mesh.primitive_cylinder_add(vertices=vertices, radius=radius, depth=depth, location=location, rotation=rotation)
    obj = bpy.context.active_object
    obj.name = name
    return obj


def add_sphere(name, radius=0.5, segments=16, rings=8, location=(0, 0, 0)):
    bpy.ops.mesh.primitive_uv_sphere_add(segments=segments, ring_count=rings, radius=radius, location=location)
    obj = bpy.context.active_object
    obj.name = name
    bpy.ops.object.shade_smooth()
    return obj


def bevel(obj, width=0.1, segments=1):
    mod = obj.modifiers.new("Bevel", "BEVEL")
    mod.width = width
    mod.segments = segments
    mod.limit_method = "ANGLE"
    return mod


def material(name, color, roughness=0.8, metallic=0.0):
    """Matériau simple (couleur RVB entre 0 et 1), réutilisé s'il existe déjà."""
    mat = bpy.data.materials.get(name)
    if mat is None:
        mat = bpy.data.materials.new(name)
        mat.use_nodes = True
        bsdf = next(n for n in mat.node_tree.nodes if n.type == "BSDF_PRINCIPLED")
        bsdf.inputs["Base Color"].default_value = (*color, 1.0)
        bsdf.inputs["Roughness"].default_value = roughness
        bsdf.inputs["Metallic"].default_value = metallic
        mat.diffuse_color = (*color, 1.0)
    return mat


def set_material(obj, mat):
    obj.data.materials.clear()
    obj.data.materials.append(mat)


# ---------------------------------------------------------------------------
# Préparation et export (normalement rien à changer)
# ---------------------------------------------------------------------------


def clear_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.unit_settings.system = "NONE"
    scene.unit_settings.scale_length = 1.0


def meshes():
    return [o for o in bpy.context.scene.objects if o.type == "MESH"]


def apply_all():
    """Applique modificateurs, rotation et échelle de chaque maillage."""
    for obj in meshes():
        bpy.ops.object.select_all(action="DESELECT")
        obj.select_set(True)
        bpy.context.view_layer.objects.active = obj
        for mod in list(obj.modifiers):
            try:
                bpy.ops.object.modifier_apply(modifier=mod.name)
            except RuntimeError as error:
                print(f"! modificateur {mod.name} de {obj.name} non appliqué : {error}")
        bpy.ops.object.transform_apply(location=False, rotation=True, scale=True)


def ground_and_center():
    """Pose l'ensemble au sol (z = 0) et le centre sur X et Y."""
    objs = meshes()
    if not objs:
        return
    corners = [obj.matrix_world @ v.co for obj in objs for v in obj.data.vertices]
    min_x, max_x = min(c.x for c in corners), max(c.x for c in corners)
    min_y, max_y = min(c.y for c in corners), max(c.y for c in corners)
    min_z = min(c.z for c in corners)
    offset = ((min_x + max_x) / 2, (min_y + max_y) / 2, min_z)
    for obj in objs:
        obj.location.x -= offset[0]
        obj.location.y -= offset[1]
        obj.location.z -= offset[2]
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.transform_apply(location=True, rotation=False, scale=False)


def report():
    """Affiche triangles et taille ; renvoie (triangles du plus gros maillage, total)."""
    objs = meshes()
    depsgraph = bpy.context.evaluated_depsgraph_get()
    total = 0
    worst = 0
    for obj in objs:
        mesh = obj.evaluated_get(depsgraph).to_mesh()
        mesh.calc_loop_triangles()
        count = len(mesh.loop_triangles)
        obj.evaluated_get(depsgraph).to_mesh_clear()
        total += count
        worst = max(worst, count)
        flag = "  <-- au-delà de la limite de Roblox" if count > MAX_TRIANGLES else ""
        print(f"  {obj.name}: {count} triangles{flag}")
    corners = [obj.matrix_world @ v.co for obj in objs for v in obj.data.vertices]
    if corners:
        size = [max(c[i] for c in corners) - min(c[i] for c in corners) for i in range(3)]
        print(f"Taille : {size[0]:.2f} x {size[1]:.2f} x {size[2]:.2f} studs (X, Y, hauteur)")
    print(f"Total : {total} triangles dans {len(objs)} maillage(s)")
    return worst, total


def export(path):
    bpy.ops.object.select_all(action="SELECT")
    if path.lower().endswith(".fbx"):
        try:
            bpy.ops.export_scene.fbx(
                filepath=path,
                use_selection=True,
                object_types={"MESH", "ARMATURE", "EMPTY"},
                use_mesh_modifiers=True,
                apply_scale_options="FBX_SCALE_ALL",
                path_mode="COPY",
                embed_textures=True,
            )
        except TypeError:
            bpy.ops.export_scene.fbx(filepath=path, use_selection=True)
    else:
        try:
            bpy.ops.export_scene.gltf(filepath=path, export_format="GLB", use_selection=True, export_apply=True)
        except TypeError:
            bpy.ops.export_scene.gltf(filepath=path, export_format="GLB")
    print(f"Exporté : {path}")


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True, help="fichier .glb ou .fbx")
    parser.add_argument("--blend", help="enregistre aussi le .blend (pour retoucher à la main)")
    args = parser.parse_args(argv)

    clear_scene()
    build()
    apply_all()
    ground_and_center()
    worst, _ = report()
    if worst > MAX_TRIANGLES:
        print(f"ERREUR : un maillage dépasse {MAX_TRIANGLES} triangles. Réduis les segments ou ajoute un modificateur DECIMATE.")
        sys.exit(1)
    if args.blend:
        bpy.ops.wm.save_as_mainfile(filepath=args.blend)
    export(args.out)


if __name__ == "__main__":
    main()
