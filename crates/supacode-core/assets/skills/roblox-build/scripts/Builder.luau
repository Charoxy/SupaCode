--!nocheck
--[[
	Builder : aides de construction pour Roblox Studio.

	Colle ce fichier en tête d'un appel execute_luau (ou d'une commande Studio), puis :

		local maison = Builder.model("Maison", workspace, { replace = true })
		local base = CFrame.new(0, 0, 0)
		Builder.floor(maison, base, 24, 20, { color = Color3.fromRGB(120, 90, 60), material = Enum.Material.WoodPlanks })
		Builder.room(maison, base, 24, 20, 12, {
			color = Color3.fromRGB(225, 215, 195), material = Enum.Material.Brick,
			openings = { front = { { at = 10, width = 4, height = 8 } }, left = { { at = 7, width = 4, height = 4, bottom = 3 } } },
		})
		Builder.gableRoof(maison, base * CFrame.new(0, 12, 0), 24, 20, 7, { color = Color3.fromRGB(150, 60, 50), material = Enum.Material.Slate })
		Builder.look(Vector3.new(0, 6, 0), 45, 18, 35)

	Conventions : unités en studs, toutes les pièces ancrées, coordonnées arrondies à 0,05.
	Pour en faire un ModuleScript, ajoute `return Builder` à la fin.
]]

local Builder = {}

local GRID = 0.05

local function snap(n: number): number
	return math.round(n / GRID) * GRID
end

function Builder.snap(v: Vector3): Vector3
	return Vector3.new(snap(v.X), snap(v.Y), snap(v.Z))
end

-- Couleur légèrement variée (± amount en luminosité) : évite l'effet copier-coller.
function Builder.vary(color: Color3, amount: number?): Color3
	local h, s, v = color:ToHSV()
	local delta = (math.random() * 2 - 1) * (amount or 0.06)
	return Color3.fromHSV(h, s, math.clamp(v * (1 + delta), 0, 1))
end

-- Modèle conteneur. replace = true supprime l'ancien modèle du même nom.
function Builder.model(name: string, parent: Instance?, opts: { replace: boolean? }?): Model
	parent = parent or workspace
	local existing = parent:FindFirstChild(name)
	if existing and opts and opts.replace then
		existing:Destroy()
		existing = nil
	end
	if existing and existing:IsA("Model") then
		return existing
	end
	local model = Instance.new("Model")
	model.Name = name
	model.Parent = parent
	return model
end

function Builder.folder(name: string, parent: Instance): Folder
	local folder = parent:FindFirstChild(name) or Instance.new("Folder")
	folder.Name = name
	folder.Parent = parent
	return folder
end

--[[
	Pièce générique. opts :
	  size, cframe, color, material, name, transparency, reflectance
	  class = "Part" | "WedgePart" | "CornerWedgePart" | "TrussPart"
	  shape = Enum.PartType.Cylinder | Ball (Part seulement ; l'axe du cylindre est X)
	  deco = true : pas de collision, ni de contact, ni de requête ; pas d'ombre si petite
]]
function Builder.part(parent: Instance, opts): BasePart
	local part = Instance.new(opts.class or "Part")
	part.Anchored = true
	part.TopSurface = Enum.SurfaceType.Smooth
	part.BottomSurface = Enum.SurfaceType.Smooth
	if opts.shape and part:IsA("Part") then
		part.Shape = opts.shape
	end
	local size = opts.size or Vector3.one
	part.Size = Vector3.new(math.max(snap(size.X), 0.05), math.max(snap(size.Y), 0.05), math.max(snap(size.Z), 0.05))
	local cf = opts.cframe or CFrame.new()
	part.CFrame = CFrame.new(Builder.snap(cf.Position)) * cf.Rotation
	part.Color = opts.color or Color3.fromRGB(200, 200, 200)
	part.Material = opts.material or Enum.Material.SmoothPlastic
	part.Transparency = opts.transparency or 0
	part.Reflectance = opts.reflectance or 0
	part.Name = opts.name or part.ClassName
	if opts.deco then
		part.CanCollide = false
		part.CanTouch = false
		part.CanQuery = false
		if part.Size.Magnitude < 4 then
			part.CastShadow = false
		end
	end
	part.Parent = parent
	return part
end

-- Pavé : raccourci de Builder.part.
function Builder.box(parent: Instance, size: Vector3, cframe: CFrame, color: Color3?, material: Enum.Material?, extra)
	local opts = extra and table.clone(extra) or {}
	opts.size, opts.cframe, opts.color, opts.material = size, cframe, color or opts.color, material or opts.material
	return Builder.part(parent, opts)
end

--[[
	Mur droit entre deux points au sol (from, to : Vector3, même altitude), de hauteur height.
	opts : thickness (1), color, material, name,
	       openings = { { at = distance depuis from, width, height, bottom = 0 } }  -- portes, fenêtres
	       frame = { color, material, depth = 0.3, width = 0.4 } : encadrement des ouvertures
	Renvoie la liste des pièces.
]]
function Builder.wall(parent: Instance, from: Vector3, to: Vector3, height: number, opts)
	opts = opts or {}
	local thickness = opts.thickness or 1
	local flat = Vector3.new(to.X - from.X, 0, to.Z - from.Z)
	local length = flat.Magnitude
	if length < 0.05 then
		return {}
	end
	local cf = CFrame.lookAt(from, from + flat.Unit) -- -Z va de from vers to
	local parts = {}
	local function piece(d0, d1, y0, y1)
		if d1 - d0 < 0.05 or y1 - y0 < 0.05 then
			return
		end
		table.insert(parts, Builder.part(parent, {
			name = opts.name or "Mur",
			size = Vector3.new(thickness, y1 - y0, d1 - d0),
			cframe = cf * CFrame.new(0, (y0 + y1) / 2, -(d0 + d1) / 2),
			color = opts.color,
			material = opts.material,
		}))
	end
	local openings = table.clone(opts.openings or {})
	table.sort(openings, function(a, b)
		return a.at < b.at
	end)
	local cursor = 0
	for _, o in openings do
		local bottom = o.bottom or 0
		local top = math.min(bottom + o.height, height)
		piece(cursor, o.at, 0, height)
		piece(o.at, o.at + o.width, 0, bottom)
		piece(o.at, o.at + o.width, top, height)
		cursor = o.at + o.width
		if opts.frame then
			local f = opts.frame
			local depth, w = f.depth or 0.3, f.width or 0.4
			local center = o.at + o.width / 2
			for _, side in { -1, 1 } do
				-- Montants, en saillie des deux côtés du mur.
				table.insert(parts, Builder.part(parent, {
					name = "Encadrement", deco = true, color = f.color, material = f.material,
					size = Vector3.new(thickness + depth * 2, top - bottom, w),
					cframe = cf * CFrame.new(0, (bottom + top) / 2, -(center + side * (o.width / 2 + w / 2))),
				}))
			end
			table.insert(parts, Builder.part(parent, {
				name = "Linteau", deco = true, color = f.color, material = f.material,
				size = Vector3.new(thickness + depth * 2, w, o.width + w * 2),
				cframe = cf * CFrame.new(0, top + w / 2, -center),
			}))
			if bottom > 0 then
				table.insert(parts, Builder.part(parent, {
					name = "Rebord", deco = true, color = f.color, material = f.material,
					size = Vector3.new(thickness + depth * 3, w, o.width + w * 2),
					cframe = cf * CFrame.new(0, bottom - w / 2, -center),
				}))
			end
		end
	end
	piece(cursor, length, 0, height)
	return parts
end

--[[
	Pièce rectangulaire fermée, sans recouvrement aux angles (pas de scintillement).
	cframe : centre du sol de la pièce. width le long de X local, depth le long de Z local.
	opts : comme Builder.wall, plus openings = { front = {...}, back = {...}, left = {...}, right = {...} }
	       (front est du côté -Z local ; "at" se mesure depuis le coin gauche vu de l'extérieur).
]]
function Builder.room(parent: Instance, cframe: CFrame, width: number, depth: number, height: number, opts)
	opts = opts or {}
	local t = opts.thickness or 1
	local w, d = width / 2, depth / 2
	local function at(x, z)
		return cframe:PointToWorldSpace(Vector3.new(x, 0, z))
	end
	local openings = opts.openings or {}
	local function side(name, a, b)
		local o = table.clone(opts)
		o.openings = openings[name]
		o.name = "Mur" .. name
		return Builder.wall(parent, a, b, height, o)
	end
	side("front", at(-w - t / 2, -d), at(w + t / 2, -d))
	side("back", at(w + t / 2, d), at(-w - t / 2, d))
	side("left", at(-w, d - t / 2), at(-w, -d + t / 2))
	side("right", at(w, -d + t / 2), at(w, d - t / 2))
end

-- Dalle de sol (le dessus est à l'altitude de cframe).
function Builder.floor(parent: Instance, cframe: CFrame, width: number, depth: number, opts)
	opts = opts or {}
	local thickness = opts.thickness or 1
	return Builder.part(parent, {
		name = opts.name or "Sol", color = opts.color, material = opts.material,
		size = Vector3.new(width + (opts.margin or 0) * 2, thickness, depth + (opts.margin or 0) * 2),
		cframe = cframe * CFrame.new(0, -thickness / 2, 0),
	})
end

--[[
	Toit à deux pans. cframe : centre du haut des murs. width : portée (X local), length :
	longueur du faîtage (Z local), rise : hauteur du faîtage.
	opts : overhang (1,2), thickness (0,6), color, material (pans),
	       gable = { color, material } : pignons triangulaires (par défaut : comme les pans),
	       ridge = true : faîtière.
]]
function Builder.gableRoof(parent: Instance, cframe: CFrame, width: number, length: number, rise: number, opts)
	opts = opts or {}
	local overhang = opts.overhang or 1.2
	local thickness = opts.thickness or 0.6
	local half = width / 2
	local angle = math.atan2(rise, half)
	local slope = (half + overhang) / math.cos(angle)
	local roofLength = length + overhang * 2
	local ridge = Vector3.new(0, rise, 0)
	for _, s in { -1, 1 } do
		local down = Vector3.new(s * math.cos(angle), -math.sin(angle), 0)
		local normal = Vector3.new(s * math.sin(angle), math.cos(angle), 0)
		local center = ridge + down * (slope / 2) + normal * (thickness / 2)
		local local_cf = CFrame.fromMatrix(center, Vector3.zAxis, normal)
		Builder.part(parent, {
			name = "Toit", color = opts.color, material = opts.material,
			size = Vector3.new(roofLength, thickness, slope),
			cframe = cframe * local_cf,
		})
	end
	-- Pignons : deux demi-triangles (WedgePart) à chaque bout, côté haut vers le centre, dans
	-- l'axe des murs d'extrémité (ceux de Builder.room sont centrés sur ±length / 2).
	local gable = opts.gable or { color = opts.color, material = opts.material }
	local wallThickness = gable.thickness or 1
	for _, z in { -length / 2, length / 2 } do
		for _, s in { -1, 1 } do
			Builder.part(parent, {
				class = "WedgePart", name = "Pignon", color = gable.color, material = gable.material,
				size = Vector3.new(wallThickness, rise, half),
				cframe = cframe * CFrame.new(s * half / 2, rise / 2, z) * CFrame.Angles(0, math.rad(-90 * s), 0),
			})
		end
	end
	if opts.ridge ~= false then
		Builder.part(parent, {
			name = "Faitiere", color = opts.ridgeColor or opts.color, material = opts.material,
			size = Vector3.new(thickness * 1.6, thickness * 1.2, roofLength + 0.1),
			cframe = cframe * CFrame.new(0, rise + thickness * 0.6, 0),
		})
	end
end

-- Escalier plein. cframe : milieu du bas de la première marche, on monte vers -Z local.
function Builder.stairs(parent: Instance, cframe: CFrame, width: number, steps: number, opts)
	opts = opts or {}
	local rise, run = opts.rise or 1, opts.run or 1.6
	for i = 1, steps do
		Builder.part(parent, {
			name = "Marche" .. i, color = opts.vary and Builder.vary(opts.color or Color3.fromRGB(160, 160, 160), 0.04) or opts.color,
			material = opts.material,
			size = Vector3.new(width, rise * i, run),
			cframe = cframe * CFrame.new(0, rise * i / 2, -run * (i - 0.5)),
		})
	end
end

-- Clôture ou garde-corps entre deux points au sol.
function Builder.fence(parent: Instance, from: Vector3, to: Vector3, opts)
	opts = opts or {}
	local height, spacing, post = opts.height or 3, opts.spacing or 4, opts.post or 0.5
	local rails = opts.rails or 2
	local flat = Vector3.new(to.X - from.X, 0, to.Z - from.Z)
	local length = flat.Magnitude
	local cf = CFrame.lookAt(from, from + flat.Unit)
	local count = math.max(1, math.round(length / spacing))
	for i = 0, count do
		Builder.part(parent, {
			name = "Poteau", color = opts.color, material = opts.material,
			size = Vector3.new(post, height, post),
			cframe = cf * CFrame.new(0, height / 2, -length * i / count),
		})
	end
	for r = 1, rails do
		local y = height * r / (rails + 0.5)
		Builder.part(parent, {
			name = "Lisse", deco = opts.deco, color = opts.color, material = opts.material,
			size = Vector3.new(post * 0.5, post * 0.5, length),
			cframe = cf * CFrame.new(0, y, -length / 2),
		})
	end
end

-- Répète fn(i, t) count fois ; t va de 0 à 1.
function Builder.array(count: number, fn)
	for i = 1, count do
		fn(i, count > 1 and (i - 1) / (count - 1) or 0)
	end
end

-- Lumière chaude dans une pièce (lampe, lanterne).
function Builder.light(part: BasePart, opts)
	opts = opts or {}
	local light = Instance.new(opts.spot and "SpotLight" or "PointLight")
	light.Color = opts.color or Color3.fromRGB(255, 200, 140)
	light.Brightness = opts.brightness or 1.5
	light.Range = opts.range or 16
	light.Shadows = opts.shadows or false
	light.Parent = part
	return light
end

-- Place la caméra de Studio pour une capture : vers target, à distance, hauteur et angle (degrés).
function Builder.look(target: Vector3, distance: number, height: number, angle: number?)
	local a = math.rad(angle or 35)
	local position = target + Vector3.new(math.sin(a) * distance, height, math.cos(a) * distance)
	workspace.CurrentCamera.CFrame = CFrame.lookAt(position, target)
end

-- Compte les pièces d'un modèle (budget).
function Builder.count(model: Instance): number
	local n = 0
	for _, d in model:GetDescendants() do
		if d:IsA("BasePart") then
			n += 1
		end
	end
	return n
end

-- Point d'annulation : Builder.begin("Maison") ... Builder.commit(id) ; Ctrl+Z défait tout d'un coup.
function Builder.begin(name: string)
	local ok, id = pcall(function()
		return game:GetService("ChangeHistoryService"):TryBeginRecording(name)
	end)
	return ok and id or nil
end

function Builder.commit(id)
	if id then
		pcall(function()
			game:GetService("ChangeHistoryService"):FinishRecording(id, Enum.FinishRecordingOperation.Commit)
		end)
	end
end
