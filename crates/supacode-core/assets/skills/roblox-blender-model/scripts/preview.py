"""Image d'aperçu d'un modèle exporté (.glb, .gltf, .fbx ou .obj), rendue sans interface.

    blender -b --factory-startup -P preview.py -- modele.glb apercu.png [taille]

Moteur Workbench (rapide, sans GPU dédié), vue 3/4 cadrée sur le modèle, couleurs des
matériaux, fond gris neutre.
"""

import math
import sys

import bpy
from mathutils import Vector


def import_model(path):
    lower = path.lower()
    if lower.endswith((".glb", ".gltf")):
        bpy.ops.import_scene.gltf(filepath=path)
    elif lower.endswith(".fbx"):
        if hasattr(bpy.ops.wm, "fbx_import"):
            try:
                bpy.ops.wm.fbx_import(filepath=path)
                return
            except (RuntimeError, TypeError):
                pass
        bpy.ops.import_scene.fbx(filepath=path)
    elif lower.endswith(".obj"):
        bpy.ops.wm.obj_import(filepath=path)
    else:
        raise SystemExit(f"format non pris en charge : {path}")


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if len(argv) < 2:
        raise SystemExit("usage : preview.py -- modele.glb apercu.png [taille]")
    model, output = argv[0], argv[1]
    size = int(argv[2]) if len(argv) > 2 else 768

    bpy.ops.wm.read_factory_settings(use_empty=True)
    import_model(model)
    objs = [o for o in bpy.context.scene.objects if o.type == "MESH"]
    if not objs:
        raise SystemExit("aucun maillage dans le fichier")

    corners = [o.matrix_world @ Vector(c) for o in objs for c in o.bound_box]
    low = Vector((min(c.x for c in corners), min(c.y for c in corners), min(c.z for c in corners)))
    high = Vector((max(c.x for c in corners), max(c.y for c in corners), max(c.z for c in corners)))
    center = (low + high) / 2
    radius = max((high - low).length / 2, 0.01)

    scene = bpy.context.scene
    try:
        scene.render.engine = "BLENDER_WORKBENCH"
    except TypeError:
        pass
    shading = scene.display.shading
    shading.light = "STUDIO"
    shading.color_type = "MATERIAL"
    shading.show_cavity = True
    shading.show_object_outline = True
    scene.render.resolution_x = size
    scene.render.resolution_y = size
    scene.render.film_transparent = False
    world = bpy.data.worlds.new("Fond")
    world.color = (0.18, 0.18, 0.2)
    scene.world = world

    camera_data = bpy.data.cameras.new("Camera")
    camera_data.lens = 50
    camera = bpy.data.objects.new("Camera", camera_data)
    scene.collection.objects.link(camera)
    direction = Vector((1.0, -1.3, 0.85)).normalized()
    distance = radius / math.tan(camera_data.angle / 2) * 1.15
    camera.location = center + direction * distance
    camera.rotation_euler = (center - camera.location).to_track_quat("-Z", "Y").to_euler()
    camera_data.clip_end = distance * 4
    scene.camera = camera

    scene.render.filepath = output
    bpy.ops.render.render(write_still=True)
    print(f"Aperçu : {output}")
    print(f"Taille : {high.x - low.x:.2f} x {high.y - low.y:.2f} x {high.z - low.z:.2f} (unités du fichier)")


if __name__ == "__main__":
    main()
