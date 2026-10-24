class_name HordeDirector
extends Node3D
## Distance-capped horde for NH_04. Rules match rust/gore-core/src/horde.rs.
##
## The committed layout res://spawn/nh04_fallback.json is an invented stand-in: made-up positions and
## made-up spawn tables, not a Dead Island 2 extract. A real dump, when present, is CUE4Parse JSON
## (a flat array of exports with Type, Name, Outer, Properties) under the gitignored extract directory.
## RelativeLocation is world space unless the component has an AttachParent. A missing cooked field is
## the class default, which is not in the file, so it is counted as fallback-filled.

## Fallback allow-list, used only when res://data/archetypes.json is not present (a clean checkout, CI).
##
## It is limited to these 22 because they are the P2 elemental and hazard rows already recorded in
## recon/ARCHETYPES.md (Q14), which is committed text and not game data. The base Walker / Runner /
## Shambler / Thug bodies and the specials are left out on purpose: their ids come from the generated
## table, so a checkout without it cannot vouch for them. With the table present this list is unused.
const FALLBACK_ARCHETYPE_IDS := [
	"Walker_Burning", "Runner_Burning", "Walker_Fireman", "Runner_Fireman",
	"Walker_Shocking", "Runner_Shocking", "Walker_Caustic", "Runner_Caustic",
	"Walker_Spiky", "Runner_Spiky", "Walker_Vicious", "Runner_Vicious",
	"Walker_FuelTank", "Walker_WaterTank", "Walker_Corrosive", "Walker_GrenadeVest",
	"Walker_InsectSwarm", "Walker_Radio", "Walker_RiotGear_Light", "Walker_RiotGear_Medium",
	"Walker_RiotGear_VeryLight", "Walker_RiotGear_Heavy",
]
const ARCHETYPES_JSON := "res://data/archetypes.json"
const KNOWN := ["BP_ZombieSpawnPoint_C", "BP_Horde_TSL_ZombieCluster_C", "BP_Activity_Feasting_C"]
const POPULATION := ["BP_ZombieSpawnPoint_C", "BP_Activity_Feasting_C"]
const ARCHETYPE_KEYS := ["Archetype", "ArchetypeId", "ZombieArchetype", "SpawnArchetype", "ZombieType"]
const TABLE_KEYS := ["SpawnTable", "SpawnTableId", "ZombieCluster", "Cluster"]
const EXTRACT_DIR := "extracted/json/DeadIsland/Content/DI2/Horde/Maps/NH_04"
const DUMP_FILE := "extracted/json/DeadIsland/Content/DI2/Horde/Maps/NH_04/SubLevels/NH_04_Spawning.json"
const FALLBACK := "res://spawn/nh04_fallback.json"
## Seed for the stand-in's borrowed ids. Same value and rule as STAND_IN_ID_SEED in horde.rs.
const STAND_IN_ID_SEED := 1

var running := true
var use_zombies := false
var main: Node = null
var stand_in := false
var records: Array = []
var tables := {}
var budget := {
	"max_active": 8,
	"min_spawn_distance": 8.0,
	"max_spawn_distance": 42.0,
	"despawn_distance": 64.0,
	"spawn_interval": 2.5,
	"max_spawns_per_tick": 1,
	"point_cooldown": 12.0,
	"view_half_angle_deg": 55.0,
	"seed": 1,
}
var points: Array = []
var live: Array = []
var next_id := 1
var spawn_wait := 0.0
var _rng := 1
## Stand-in id -> loaded table id for the export list being parsed. Empty unless stand-in + table.
var _id_map := {}


func _ready() -> void:
	if records.is_empty():
		load_layout()
	_rebuild_points()


func _process(delta: float) -> void:
	if not running or main == null:
		return
	var player = main.get("player")
	if player == null:
		return
	var fwd := Vector3(0, 0, -1)
	var cam = player.get("camera")
	if cam != null:
		fwd = -cam.global_basis.z
	step(delta, player.global_position, fwd)


func active() -> int:
	return live.size()


func load_layout() -> bool:
	var root := _repo_root()
	var dir := root.path_join(EXTRACT_DIR)
	if DirAccess.dir_exists_absolute(dir):
		var files: PackedStringArray = []
		_collect_json(dir, files)
		files.sort()
		records.clear()
		tables.clear()
		stand_in = false
		for path in files:
			var file_exports = _read_json_file(path)
			if typeof(file_exports) != TYPE_ARRAY:
				continue
			_parse_into(file_exports)
		if not records.is_empty():
			return true
	var dump := root.path_join(DUMP_FILE)
	if FileAccess.file_exists(dump):
		var dump_exports = _read_json_file(dump)
		if typeof(dump_exports) == TYPE_ARRAY:
			records.clear()
			tables.clear()
			stand_in = false
			_parse_into(dump_exports)
			return true
	var fallback_exports = _read_json_file(ProjectSettings.globalize_path(FALLBACK))
	if typeof(fallback_exports) != TYPE_ARRAY:
		push_error("horde: stand-in layout missing (%s)" % FALLBACK)
		return false
	records.clear()
	tables.clear()
	stand_in = false
	_parse_into(fallback_exports)
	return true


func step(dt: float, player: Vector3, forward: Vector3) -> void:
	for p in points:
		p["cooldown"] = max(0.0, float(p["cooldown"]) - dt)
	var despawn_at := float(budget["despawn_distance"])
	var gone: Array = []
	for z in live:
		if _flat(z["position"], player) > despawn_at:
			gone.append(int(z["id"]))
	for id in gone:
		_remove_live(int(id))
	spawn_wait -= dt
	if spawn_wait > 0.0 or int(budget["max_spawns_per_tick"]) == 0:
		return
	var room := int(budget["max_active"]) - live.size()
	var n: int = mini(maxi(room, 0), int(budget["max_spawns_per_tick"]))
	var spawned := 0
	for _i in n:
		if _try_spawn(player, forward):
			spawned += 1
		else:
			break
	if spawned > 0:
		spawn_wait = float(budget["spawn_interval"])


func notify_gone(id: int) -> void:
	_remove_live(id)


func _remove_live(id: int) -> void:
	for i in live.size():
		if int(live[i]["id"]) != id:
			continue
		var record_id: String = live[i]["record_id"]
		var node: Node = live[i]["node"]
		if is_instance_valid(node):
			node.queue_free()
		live.remove_at(i)
		for p in points:
			if p["record_id"] == record_id:
				p["occupant"] = -1
				p["cooldown"] = float(budget["point_cooldown"])
				break
		return


func _try_spawn(player: Vector3, forward: Vector3) -> bool:
	var min_d := float(budget["min_spawn_distance"])
	var max_d := float(budget["max_spawn_distance"])
	var candidates: Array[int] = []
	for point_i in points.size():
		var p = points[point_i]
		if int(p["occupant"]) >= 0 or float(p["cooldown"]) > 0.0:
			continue
		var d := _flat(p["position"], player)
		if d < min_d or d > max_d:
			continue
		if not _can_spawn(point_i):
			continue
		candidates.append(point_i)
	if candidates.is_empty():
		return false
	var cos_half := cos(deg_to_rad(float(budget["view_half_angle_deg"])))
	var hidden: Array[int] = []
	for hidden_i in candidates:
		if not _in_view(player, forward, points[hidden_i]["position"], cos_half):
			hidden.append(hidden_i)
	var pool: Array[int] = hidden if not hidden.is_empty() else candidates
	var weights: Array[float] = []
	for pool_i in pool:
		weights.append(1.0 / max(_flat(points[pool_i]["position"], player), 1.0))
	var pick := pool[_pick(weights)]
	var archetype := _archetype_for(pick)
	if archetype == "":
		return false
	var id := next_id
	next_id += 1
	var pos: Vector3 = points[pick]["position"]
	var yaw := float(points[pick]["yaw"])
	points[pick]["occupant"] = id
	var node := _visual(pos, yaw, archetype, id)
	live.append({"id": id, "record_id": points[pick]["record_id"], "archetype": archetype, "position": pos, "node": node})
	return true


func _can_spawn(index: int) -> bool:
	if _allowed(str(points[index]["archetype_id"])):
		return true
	var w = tables.get(str(points[index]["table"]), [])
	return typeof(w) == TYPE_ARRAY and not w.is_empty()


func _archetype_for(index: int) -> String:
	var own := str(points[index]["archetype_id"])
	if _allowed(own):
		return own
	var w = tables.get(str(points[index]["table"]), [])
	if typeof(w) != TYPE_ARRAY or w.is_empty():
		return ""
	var weights: Array[float] = []
	for pair in w:
		weights.append(float(pair[1]))
	return str(w[_pick(weights)][0])


func _visual(pos: Vector3, yaw: float, archetype: String, id: int) -> Node3D:
	var can := use_zombies and main != null and main.has_method("spawn_zombie") \
		and ResourceLoader.exists("res://assets/di2/walker/walker.glb") and ClassDB.class_exists("GoreBody")
	if can:
		var z: Node3D = main.spawn_zombie(pos, archetype)
		z.rotation.y = deg_to_rad(yaw)
		z.add_to_group("horde_spawns")
		if z.has_signal("died"):
			z.died.connect(func(_zombie): notify_gone(id))
		return z
	var body := MeshInstance3D.new()
	var mesh := CapsuleMesh.new()
	mesh.radius = 0.35
	mesh.height = 1.8
	var mat := StandardMaterial3D.new()
	mat.albedo_color = Color(0.42, 0.52, 0.3)
	mesh.material = mat
	body.mesh = mesh
	body.position = pos
	body.rotation.y = deg_to_rad(yaw)
	body.name = "Horde_%d" % id
	add_child(body)
	body.add_to_group("horde_spawns")
	return body


func _rebuild_points() -> void:
	points.clear()
	for r in records:
		if r["unknown"] or not POPULATION.has(r["type"]):
			continue
		var arch := str(r["archetype_id"])
		points.append({
			"record_id": r["id"],
			"position": _to_godot(r["location"]),
			"yaw": float(r["rotation"].y),
			"archetype_id": arch if _allowed(arch) else "",
			"table": r["table"],
			"cooldown": 0.0,
			"occupant": -1,
		})
	_rng = maxi(int(budget["seed"]), 1)
	spawn_wait = 0.0


func _parse_into(exports: Array) -> void:
	var start := records.size()
	var local_tables := {}
	_id_map = {}
	for e0 in exports:
		if typeof(e0) == TYPE_DICTIONARY and _is_stand_in(e0):
			_id_map = stand_in_id_map()
			break
	for i in exports.size():
		var e = exports[i]
		if typeof(e) != TYPE_DICTIONARY:
			continue
		var ty := str(e.get("Type", ""))
		var name := str(e.get("Name", ""))
		var props = e.get("Properties", {})
		if typeof(props) != TYPE_DICTIONARY:
			props = {}
		if _is_stand_in(e):
			stand_in = true
		if ty == "HordeDirectorBudget":
			_apply_budget(props)
			continue
		if not _candidate(ty, props):
			continue
		var placed = _actor_transform(exports, i)
		var arch = _archetype_of(props)
		var table = _table_link(exports, props)
		var spawn_table: String = table["name"]
		var resolved: bool = table["resolved"]
		var weights = _weights(props)
		if not weights.is_empty():
			local_tables[name] = weights
			if not tables.has(name):
				tables[name] = weights
		if spawn_table == "" and local_tables.has(name):
			spawn_table = name
			resolved = true
		records.append({
			"id": name,
			"type": ty,
			"unknown": not KNOWN.has(ty),
			"location": placed["location"],
			"rotation": placed["rotation"],
			"archetype_id": arch["id"],
			"archetype_raw": arch["raw"],
			"table": spawn_table,
			"table_resolved": resolved,
			"fallbacks": placed["fallbacks"],
			"archetype_missing": arch["missing"],
			"table_present": table["present"],
		})
	for export_row in exports:
		if typeof(export_row) != TYPE_DICTIONARY or str(export_row.get("Type", "")) != "BP_Horde_TSL_ZombieCluster_C":
			continue
		var cluster := str(export_row.get("Name", ""))
		var cprops = export_row.get("Properties", {})
		if typeof(cprops) != TYPE_DICTIONARY:
			continue
		var list = cprops.get("SpawnPoints", [])
		if typeof(list) != TYPE_ARRAY:
			continue
		for r in list:
			var idx := _ref_index(exports, r)
			if idx < 0:
				continue
			var point_name := str(exports[idx].get("Name", ""))
			for link_i in range(start, records.size()):
				var linked = records[link_i]
				if linked["id"] == point_name and str(linked["table"]) == "":
					linked["table"] = cluster
					linked["table_resolved"] = true
	for done_i in range(start, records.size()):
		var done = records[done_i]
		if str(done["table"]) == "" and not done["table_resolved"] and not done["table_present"]:
			done["fallbacks"].append("SpawnTable")
		if done["archetype_missing"]:
			done["fallbacks"].append("Archetype")


func _apply_budget(props: Dictionary) -> void:
	if props.has("MaxActive"):
		budget["max_active"] = int(props["MaxActive"])
	if props.has("MinSpawnDistance"):
		budget["min_spawn_distance"] = float(props["MinSpawnDistance"])
	if props.has("MaxSpawnDistance"):
		budget["max_spawn_distance"] = float(props["MaxSpawnDistance"])
	if props.has("DespawnDistance"):
		budget["despawn_distance"] = float(props["DespawnDistance"])
	if props.has("SpawnInterval"):
		budget["spawn_interval"] = float(props["SpawnInterval"])
	if props.has("MaxSpawnsPerTick"):
		budget["max_spawns_per_tick"] = int(props["MaxSpawnsPerTick"])
	if props.has("PointCooldown"):
		budget["point_cooldown"] = float(props["PointCooldown"])
	if props.has("ViewHalfAngleDeg"):
		budget["view_half_angle_deg"] = float(props["ViewHalfAngleDeg"])
	if props.has("Seed"):
		budget["seed"] = int(props["Seed"])


func _candidate(ty: String, props: Dictionary) -> bool:
	if ty == "" or ty.contains("Component") or ty == "HordeDirectorBudget":
		return false
	return props.has("RootComponent") or ty.ends_with("_C")


func _archetype_of(props: Dictionary) -> Dictionary:
	for key in ARCHETYPE_KEYS:
		if not props.has(key):
			continue
		var raw := _prop_text(props[key])
		if raw == "":
			continue
		var mapped := str(_id_map.get(raw, raw))
		return {"id": mapped if _allowed(mapped) else "", "raw": raw, "missing": false}
	return {"id": "", "raw": "", "missing": true}


func _table_link(exports: Array, props: Dictionary) -> Dictionary:
	for key in TABLE_KEYS:
		if not props.has(key):
			continue
		var v = props[key]
		if typeof(v) == TYPE_STRING:
			if str(v) != "":
				return {"name": str(v), "resolved": true, "present": true}
			return {"name": "", "resolved": false, "present": true}
		var idx := _ref_index(exports, v)
		if idx >= 0:
			var name := str(exports[idx].get("Name", ""))
			if name != "":
				return {"name": name, "resolved": true, "present": true}
		return {"name": "", "resolved": false, "present": true}
	return {"name": "", "resolved": false, "present": false}


func _weights(props: Dictionary) -> Array:
	if not props.has("ArchetypeWeights"):
		return []
	var v = props["ArchetypeWeights"]
	var out: Array = []
	var key := ""
	var val := 0.0
	if typeof(v) == TYPE_ARRAY:
		for item in v:
			if typeof(item) != TYPE_DICTIONARY:
				continue
			key = str(item.get("Key", ""))
			key = str(_id_map.get(key, key))
			val = float(item.get("Value", 0))
			if key != "" and val > 0.0 and _allowed(key):
				out.append([key, val])
	elif typeof(v) == TYPE_DICTIONARY:
		for raw_key in v.keys():
			key = str(_id_map.get(str(raw_key), str(raw_key)))
			val = float(v[raw_key])
			if val > 0.0 and _allowed(key):
				out.append([key, val])
	return out


func _actor_transform(exports: Array, actor: int) -> Dictionary:
	var props = exports[actor].get("Properties", {})
	if typeof(props) != TYPE_DICTIONARY or not props.has("RootComponent"):
		return {"location": Vector3.ZERO, "rotation": Vector3.ZERO, "fallbacks": ["RootComponent"]}
	var root := _ref_index(exports, props["RootComponent"])
	if root < 0:
		return {"location": Vector3.ZERO, "rotation": Vector3.ZERO, "fallbacks": ["RootComponent"]}
	return _component_world(exports, root, 0)


func _component_world(exports: Array, index: int, depth: int) -> Dictionary:
	if depth > 16 or index < 0 or index >= exports.size():
		return {"location": Vector3.ZERO, "rotation": Vector3.ZERO, "rows": _ident(), "fallbacks": []}
	var export = exports[index]
	var cname := str(export.get("Name", "component"))
	var props = export.get("Properties", {})
	if typeof(props) != TYPE_DICTIONARY:
		props = {}
	var loc = _read_vec(props, "RelativeLocation", ["X", "Y", "Z"])
	var rot = _read_vec(props, "RelativeRotation", ["Pitch", "Yaw", "Roll"])
	var fallbacks: Array = []
	for miss_loc in loc["missing"]:
		fallbacks.append("%s.%s" % [cname, miss_loc])
	for miss_rot in rot["missing"]:
		fallbacks.append("%s.%s" % [cname, miss_rot])
	var rows := _rot_rows(rot["v"])
	if props.has("AttachParent"):
		var parent := _ref_index(exports, props["AttachParent"])
		if parent >= 0 and parent != index:
			var up = _component_world(exports, parent, depth + 1)
			var rotated := _mul_rows(up["rows"], loc["v"])
			for f in up["fallbacks"]:
				fallbacks.append(f)
			return {
				"location": rotated + up["location"],
				"rotation": rot["v"],
				"rows": _mat_mul(rows, up["rows"]),
				"fallbacks": fallbacks,
			}
		fallbacks.append("%s.AttachParent" % cname)
	return {"location": loc["v"], "rotation": rot["v"], "rows": rows, "fallbacks": fallbacks}


func _to_godot(loc: Vector3) -> Vector3:
	var ox := 0.0
	var oy := 0.0
	var oz := 0.0
	var yaw := 0.0
	if not stand_in:
		ox = -311.0
		oy = -70472.0
		oz = 809.4999387264252
		yaw = 90.0
	var dx := (loc.x - ox) * 0.01
	var dy := (loc.y - oy) * 0.01
	var dz := (loc.z - oz) * 0.01
	var sx := dx
	var sy := dz
	var sz := dy
	var a := deg_to_rad(yaw)
	var c := cos(a)
	var s := sin(a)
	return Vector3(c * sx + s * sz, sy, -s * sx + c * sz)


func _rot_rows(deg: Vector3) -> Array:
	var p := deg_to_rad(deg.x)
	var y := deg_to_rad(deg.y)
	var ro := deg_to_rad(deg.z)
	var sp := sin(p)
	var cp := cos(p)
	var sy := sin(y)
	var cy := cos(y)
	var sr := sin(ro)
	var cr := cos(ro)
	return [
		Vector3(cp * cy, cp * sy, sp),
		Vector3(sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp),
		Vector3(-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp),
	]


func _ident() -> Array:
	return [Vector3(1, 0, 0), Vector3(0, 1, 0), Vector3(0, 0, 1)]


func _mat_mul(a: Array, b: Array) -> Array:
	var out: Array = []
	for i in 3:
		out.append(a[i].x * b[0] + a[i].y * b[1] + a[i].z * b[2])
	return out


func _mul_rows(rows: Array, v: Vector3) -> Vector3:
	return v.x * rows[0] + v.y * rows[1] + v.z * rows[2]


func _read_vec(props: Dictionary, key: String, axes: Array) -> Dictionary:
	if not props.has(key) or typeof(props[key]) != TYPE_DICTIONARY:
		return {"v": Vector3.ZERO, "missing": [key]}
	var src: Dictionary = props[key]
	var v := Vector3.ZERO
	var missing: Array = []
	for i in 3:
		if src.has(axes[i]):
			v[i] = float(src[axes[i]])
		else:
			missing.append("%s.%s" % [key, axes[i]])
	return {"v": v, "missing": missing}


func _ref_index(exports: Array, reference) -> int:
	if typeof(reference) != TYPE_DICTIONARY:
		return -1
	var path := str(reference.get("ObjectPath", ""))
	var dot := path.rfind(".")
	if dot >= 0 and path.substr(dot + 1).is_valid_int():
		var index := int(path.substr(dot + 1))
		if index >= 0 and index < exports.size():
			return index
	var oname := str(reference.get("ObjectName", ""))
	var parts := oname.split("'")
	if parts.size() >= 2 and parts[parts.size() - 2] != "":
		var inner: String = parts[parts.size() - 2]
		for name_i in exports.size():
			if typeof(exports[name_i]) == TYPE_DICTIONARY and str(exports[name_i].get("Name", "")) == inner:
				return name_i
	return -1


func _prop_text(v) -> String:
	if typeof(v) == TYPE_STRING:
		return str(v)
	if typeof(v) != TYPE_DICTIONARY:
		return ""
	if v.has("ObjectName"):
		var parts := str(v["ObjectName"]).split("'")
		var inner := parts[parts.size() - 2] if parts.size() >= 2 else str(v["ObjectName"])
		if inner.ends_with("_C"):
			inner = inner.substr(0, inner.length() - 2)
		if inner.begins_with("BP_"):
			inner = inner.substr(3)
		return inner
	if v.has("ObjectPath"):
		var tail := str(v["ObjectPath"]).split("/")[-1]
		return tail.split(".")[0]
	return ""


## Archetype ids, read once like horde.rs: every row's "name" in archetypes.json, else the fallback list.
static var _archetype_ids: Array = []
static var archetype_source := ""
static var archetype_from_table := false


static func archetype_ids() -> Array:
	if not _archetype_ids.is_empty():
		return _archetype_ids
	var reason := "archetypes.json not found"
	if FileAccess.file_exists(ARCHETYPES_JSON):
		var parsed = JSON.parse_string(FileAccess.get_file_as_string(ARCHETYPES_JSON))
		if parsed is Array:
			for row in parsed:
				if row is Dictionary and row.has("name"):
					_archetype_ids.append(str(row["name"]))
			reason = "archetypes.json has no rows"
		else:
			reason = "archetypes.json does not parse"
	if _archetype_ids.is_empty():
		_archetype_ids = FALLBACK_ARCHETYPE_IDS.duplicate()
		archetype_source = "%d fallback (%s)" % [_archetype_ids.size(), reason]
	else:
		archetype_from_table = true
		archetype_source = "%d from %s" % [_archetype_ids.size(), ARCHETYPES_JSON]
	return _archetype_ids


static func _is_stand_in(e: Dictionary) -> bool:
	var props = e.get("Properties", {})
	if typeof(props) == TYPE_DICTIONARY and props.get("StandIn", false) == true:
		return true
	return str(e.get("Name", "")) == "INVENTED_STAND_IN"


## Only when the ids came from a loaded table with rows: the fallback id at index i maps to row
## i % len of the table ids sorted by name and rotated left by STAND_IN_ID_SEED % len. Same rule as
## stand_in_id_map in horde.rs. Applied to the invented stand-in only, never to a real dump.
static func stand_in_id_map() -> Dictionary:
	var ids := archetype_ids()
	if not archetype_from_table or ids.is_empty():
		return {}
	var order: Array = []
	for id in ids:
		if not order.has(id):
			order.append(id)
	order.sort()
	var k := STAND_IN_ID_SEED % order.size()
	order = order.slice(k) + order.slice(0, k)
	var map := {}
	for i in FALLBACK_ARCHETYPE_IDS.size():
		map[FALLBACK_ARCHETYPE_IDS[i]] = order[i % order.size()]
	return map


func _allowed(id: String) -> bool:
	return archetype_ids().has(id)


func _flat(a: Vector3, b: Vector3) -> float:
	return Vector2(a.x - b.x, a.z - b.z).length()


func _in_view(player: Vector3, forward: Vector3, point: Vector3, cos_half: float) -> bool:
	var to := Vector3(point.x - player.x, 0, point.z - player.z)
	var fwd := Vector3(forward.x, 0, forward.z)
	if to.length_squared() < 1e-8 or fwd.length_squared() < 1e-8:
		return false
	return to.normalized().dot(fwd.normalized()) >= cos_half - 0.0001


func _pick(weights: Array) -> int:
	var total := 0.0
	for w in weights:
		total += float(w)
	if total <= 0.0 or weights.is_empty():
		return 0
	var t := _rng_f() * total
	for i in weights.size():
		t -= float(weights[i])
		if t <= 0.0:
			return i
	return weights.size() - 1


func _rng_f() -> float:
	_rng = (_rng * 1664525 + 1013904223) & 0x7fffffff
	return float(_rng) / float(0x7fffffff)


func _collect_json(dir: String, out: PackedStringArray) -> void:
	var d := DirAccess.open(dir)
	if d == null:
		return
	for sub in d.get_directories():
		if not str(sub).begins_with("."):
			_collect_json(dir.path_join(sub), out)
	for file_name in d.get_files():
		if str(file_name).ends_with(".json"):
			out.append(dir.path_join(file_name))


func _read_json_file(path: String):
	var f := FileAccess.open(path, FileAccess.READ)
	if f == null:
		return null
	return JSON.parse_string(f.get_as_text())


func _repo_root() -> String:
	var game := ProjectSettings.globalize_path("res://")
	return game.trim_suffix("/").trim_suffix("\\").get_base_dir()
