extends Node3D
## Off-screen check: spawn markers appear and the director stays inside its cap.
## The stand-in layout is invented. A built NH_04 map is used when res://assets/di2/maps/nh04/map.json exists.
##
##   RUN_TEST_ENGINE_ARGS="res://scenes/horde_test.tscn" tools/run_test.sh --hordetest=<out_dir>

const HordeScript := preload("res://scripts/horde_director.gd")

var out_dir := ""
var _failed := false


func _ready() -> void:
	for arg in OS.get_cmdline_user_args():
		if arg.begins_with("--hordetest="):
			out_dir = arg.trim_prefix("--hordetest=")
	_build_room()
	var director = HordeScript.new()
	director.running = false
	director.name = "HordeDirector"
	add_child(director)
	if director.records.is_empty():
		_fail("no spawn records")
		return
	director.budget["max_active"] = 4
	director.budget["spawn_interval"] = 0.4
	director.budget["max_spawns_per_tick"] = 1
	var player := Vector3(0, 0.1, 6)
	var forward := Vector3(0, 0, -1)
	var guard := 0
	while director.active() < 4 and guard < 40:
		director.step(0.5, player, forward)
		guard += 1
	var n := director.active()
	if n < 1 or n > 4:
		_fail("expected 1..4 spawns, got %d" % n)
		return
	for _i in 8:
		director.step(0.5, player, forward)
	if director.active() > 4:
		_fail("cap exceeded: %d" % director.active())
		return
	var shown := get_tree().get_nodes_in_group("horde_spawns").size()
	if shown < 1:
		_fail("no horde_spawns nodes")
		return
	print("CHECK horde records=%d points=%d" % [director.records.size(), director.points.size()])
	for z in director.live:
		var p: Vector3 = z["position"]
		print("SPAWN id=%d record=%s archetype=%s pos=(%.2f, %.2f, %.2f)" % [int(z["id"]), z["record_id"], z["archetype"], p.x, p.y, p.z])
	print("CHECK horde spawns=%d nodes=%d cap=4 stand_in=%s" % [director.active(), shown, director.stand_in])
	if out_dir != "":
		await get_tree().create_timer(0.2).timeout
		DirAccess.make_dir_recursive_absolute(out_dir)
		var tex := get_viewport().get_texture()
		if tex:
			var img := tex.get_image()
			if img != null and img.get_width() > 0:
				var err := img.save_png(out_dir.path_join("horde_cap.png"))
				print("CHECK horde screenshot=%s" % err)
	get_tree().quit(0)


func _build_room() -> void:
	var map_script = load("res://scripts/di2_map.gd")
	if map_script != null and map_script.call("exists", "nh04"):
		add_child(map_script.call("create", "nh04"))
		print("CHECK horde map=nh04")
	else:
		print("CHECK horde map=stand-in floor")
		var floor_body := StaticBody3D.new()
		var mesh := MeshInstance3D.new()
		var plane := PlaneMesh.new()
		plane.size = Vector2(120, 120)
		var mat := StandardMaterial3D.new()
		mat.albedo_color = Color(0.22, 0.2, 0.18)
		plane.material = mat
		mesh.mesh = plane
		floor_body.add_child(mesh)
		add_child(floor_body)
	var sun := DirectionalLight3D.new()
	sun.rotation_degrees = Vector3(-48, -30, 0)
	add_child(sun)
	var cam := Camera3D.new()
	cam.position = Vector3(0, 14, 22)
	cam.rotation_degrees = Vector3(-32, 0, 0)
	cam.current = true
	add_child(cam)


func _fail(why: String) -> void:
	_failed = true
	push_error("HORDE FAIL: " + why)
	print("HORDE FAIL: " + why)
	get_tree().quit(1)
