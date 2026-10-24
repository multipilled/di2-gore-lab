//! Director rules and the CUE4Parse record path. Fixtures are small hand-written export arrays.
//!
//! Every test here passes an explicit id table: the in-repo [`FALLBACK_ARCHETYPE_IDS`] ([`fb`]) or a
//! small invented fixture ([`fixture_table`]). None reads `ArchetypeIds::default_path()` or
//! `DI2_ARCHETYPES_JSON`, so a locally generated `game/data/archetypes.json` cannot change a result.
//! The report-binary tests set `DI2_ARCHETYPES_JSON` on the child process only, always to an explicit
//! path. The one test that uses the real table is gated behind the `game-data` feature.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use glam::Vec3;
use gore_core::horde::{
    format_report, load_nh04_with, parse_layout_str_with, scan_dir_with, stand_in_id_map, tile_role, Budget,
    HordeDirector, HordeOrder, SpawnTransform, NH04_ORIGIN_CM, NH04_YAW_DEG, FALLBACK_ARCHETYPE_IDS, REPORT_HEADER,
};
use gore_core::horde::{ArchetypeIdSource, ArchetypeIds, SpawnLayout};
use serde_json::{json, Value};

/// The in-repo fallback set, with the reason the report binary prints when it has no table.
fn fb() -> ArchetypeIds {
    ArchetypeIds::fallback("archetypes.json not found")
}

/// A loaded-table stand-in with invented ids only (not Dead Island 2 names).
fn fixture_table() -> ArchetypeIds {
    ArchetypeIds {
        ids: ["Invented_Alpha", "Invented_Bravo", "Invented_Charlie"].iter().map(|s| s.to_string()).collect(),
        source: ArchetypeIdSource::Table(PathBuf::from("fixture/archetypes.json")),
    }
}

fn parse(text: &str) -> SpawnLayout {
    parse_layout_str_with(text, &fb()).unwrap()
}

/// Every spawn of a director run from the usual test position with no cooldown.
fn spawned_ids(layout: &SpawnLayout, ticks: usize) -> Vec<String> {
    let mut director = HordeDirector::new(layout);
    director.budget_mut().spawn_interval = 0.0;
    director.budget_mut().point_cooldown = 0.0;
    let player = Vec3::new(0.0, 0.1, 6.0);
    let mut out = Vec::new();
    for _ in 0..ticks {
        for order in director.tick(0.5, player, Vec3::new(1.0, 0.0, 0.0)) {
            if let HordeOrder::Spawn { id, archetype_id, .. } = order {
                out.push(archetype_id);
                director.notify_gone(id);
            }
        }
    }
    out
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn by_id<'a>(layout: &'a SpawnLayout, id: &str) -> &'a gore_core::horde::SpawnRecord {
    layout.records.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no record {id}"))
}

fn vec3_of(v: &Value) -> [f64; 3] {
    [
        v.get("X").and_then(|n| n.as_f64()).unwrap_or(0.0),
        v.get("Y").and_then(|n| n.as_f64()).unwrap_or(0.0),
        v.get("Z").and_then(|n| n.as_f64()).unwrap_or(0.0),
    ]
}

fn index_of(reference: &Value) -> usize {
    let path = reference["ObjectPath"].as_str().unwrap();
    path.rsplit_once('.').unwrap().1.parse().unwrap()
}

fn fallback_text() -> String {
    fs::read_to_string(repo_root().join("game/spawn/nh04_fallback.json")).expect("stand-in json")
}

#[test]
fn fallback_allowlist_is_the_existing_22() {
    assert_eq!(FALLBACK_ARCHETYPE_IDS.len(), 22);
    let mut ids = FALLBACK_ARCHETYPE_IDS.to_vec();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 22);
    let fb = ArchetypeIds::fallback("test");
    assert!(fb.contains("Walker_Burning"));
    assert!(fb.contains("Walker_RiotGear_Heavy"));
    assert!(!fb.contains("Walker"));
    assert!(!fb.contains("NotARealZombie"));
    assert!(!fb.contains("Screamer_Electric"));
    assert!(!fb.is_table());
    assert!(stand_in_id_map(&fb).is_empty(), "the fallback set never remaps the stand-in");
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("horde-arch-{tag}-{}-{}", std::process::id(), line!()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn archetype_ids_come_from_the_table_when_present() {
    // Hand-written rows with invented names; only `name` is read, other fields are ignored.
    let dir = temp_dir("table");
    let path = dir.join("archetypes.json");
    fs::write(&path, json!([
        {"name": "Invented_Walker", "parts": []},
        {"name": "Invented_Special", "extra": {"ignored": true}},
        {"name": "Invented_Walker"}
    ]).to_string()).unwrap();
    let ids = ArchetypeIds::from_path(&path);
    assert_eq!(ids.source, ArchetypeIdSource::Table(path.clone()));
    assert_eq!(ids.ids.len(), 2);
    assert!(ids.contains("Invented_Walker"));
    assert!(ids.contains("Invented_Special"));
    assert!(!ids.contains("Walker_Burning"), "fallback rows are not merged in");
    assert_eq!(ids.describe(), format!("archetype ids: 2 from {}", path.display()));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn archetype_ids_fall_back_when_the_table_is_missing() {
    let dir = temp_dir("missing");
    let ids = ArchetypeIds::from_path(&dir.join("archetypes.json"));
    assert_eq!(ids.source, ArchetypeIdSource::Fallback("archetypes.json not found".into()));
    assert_eq!(ids.ids.len(), 22);
    assert!(ids.contains("Walker_Burning"));
    assert_eq!(ids.describe(), "archetype ids: 22 fallback (archetypes.json not found)");

    let bad = dir.join("bad.json");
    fs::write(&bad, "{\"not\":\"a list\"}").unwrap();
    let ids = ArchetypeIds::from_path(&bad);
    assert!(matches!(ids.source, ArchetypeIdSource::Fallback(_)));
    assert_eq!(ids.ids.len(), 22);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn report_binary_names_the_archetype_source() {
    let dir = temp_dir("bin");
    fs::create_dir_all(dir.join("tiles")).unwrap();
    let table = dir.join("arch.json");
    fs::write(&table, json!([{"name": "Invented_Walker"}]).to_string()).unwrap();
    let bin = env!("CARGO_BIN_EXE_horde_report");
    let out = Command::new(bin).arg(dir.join("tiles")).env("DI2_ARCHETYPES_JSON", &table).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(&format!("archetype ids: 1 from {}", table.display())), "{text}");
    let out = Command::new(bin).arg(dir.join("tiles")).env("DI2_ARCHETYPES_JSON", dir.join("nope.json")).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("archetype ids: 22 fallback (archetypes.json not found)"), "{text}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_frame_origin_lands_at_zero() {
    let t = SpawnTransform { location_cm: NH04_ORIGIN_CM, rotation_deg: [0.0; 3] };
    let g = t.to_godot(NH04_ORIGIN_CM, NH04_YAW_DEG);
    assert!(g.length() < 1e-4, "{g:?}");
}

#[test]
fn tile_names_cover_fixed_dynamic_and_lighting() {
    assert_eq!(tile_role("NH_04_T01_Dynamic.json"), Some("Dynamic"));
    assert_eq!(tile_role("NH_04_T07_Fixed.json"), Some("Fixed"));
    assert_eq!(tile_role("maps/NH_04_T12_Lighting.json"), Some("Lighting"));
    assert_eq!(tile_role("NH_04_T02_Effects.json"), None);
    assert_eq!(tile_role("NH_04_Spawning.json"), None);
}

#[test]
fn director_respects_the_active_cap() {
    let layout = ring_layout(json!({
        "MaxActive": 2,
        "MinSpawnDistance": 8,
        "MaxSpawnDistance": 42,
        "DespawnDistance": 64,
        "SpawnInterval": 0,
        "MaxSpawnsPerTick": 5,
        "PointCooldown": 12,
        "ViewHalfAngleDeg": 180,
        "Seed": 7
    }));
    let mut director = HordeDirector::new(&layout);
    let player = Vec3::new(0.0, 0.1, 6.0);
    let orders = director.tick(0.5, player, Vec3::new(0.0, 0.0, 1.0));
    let spawns = orders.iter().filter(|o| matches!(o, HordeOrder::Spawn { .. })).count();
    assert_eq!(spawns, 2);
    assert_eq!(director.active(), 2);
    let again = director.tick(0.5, player, Vec3::new(0.0, 0.0, 1.0));
    assert!(again.iter().all(|o| matches!(o, HordeOrder::Despawn { .. })));
    assert_eq!(director.active(), 2);
}

#[test]
fn director_paces_spawns() {
    let layout = ring_layout(json!({
        "MaxActive": 4,
        "MinSpawnDistance": 8,
        "MaxSpawnDistance": 42,
        "DespawnDistance": 64,
        "SpawnInterval": 5,
        "MaxSpawnsPerTick": 1,
        "PointCooldown": 12,
        "ViewHalfAngleDeg": 180,
        "Seed": 3
    }));
    let mut director = HordeDirector::new(&layout);
    let player = Vec3::new(0.0, 0.1, 6.0);
    let fwd = Vec3::new(0.0, 0.0, 1.0);
    let first = director.tick(0.5, player, fwd);
    assert_eq!(spawn_count(&first), 1);
    assert_eq!(director.active(), 1);
    let held = director.tick(1.0, player, fwd);
    assert_eq!(spawn_count(&held), 0);
    assert_eq!(director.active(), 1);
    let second = director.tick(4.0, player, fwd);
    assert_eq!(spawn_count(&second), 1);
    assert_eq!(director.active(), 2);

    director.budget_mut().max_spawns_per_tick = 0;
    director.budget_mut().spawn_interval = 0.0;
    let none = director.tick(1.0, player, fwd);
    assert_eq!(spawn_count(&none), 0);
}

#[test]
fn director_despawns_past_the_cap_distance() {
    let layout = ring_layout(json!({
        "MaxActive": 2, "MinSpawnDistance": 8, "MaxSpawnDistance": 42, "DespawnDistance": 64,
        "SpawnInterval": 0, "MaxSpawnsPerTick": 2, "PointCooldown": 0, "ViewHalfAngleDeg": 180, "Seed": 1
    }));
    let mut director = HordeDirector::new(&layout);
    let player = Vec3::new(0.0, 0.1, 6.0);
    director.tick(0.5, player, Vec3::Z);
    assert_eq!(director.active(), 2);
    let gone = director.tick(0.5, Vec3::new(0.0, 0.1, 400.0), Vec3::Z);
    assert_eq!(gone.iter().filter(|o| matches!(o, HordeOrder::Despawn { .. })).count(), 2);
    assert_eq!(director.active(), 0);
}

#[test]
fn fixed_seed_repeats_and_a_different_seed_does_not() {
    let layout = weighted_layout(0.0);
    let play = |seed: u64| {
        let mut director = HordeDirector::with_seed(&layout, seed);
        let player = Vec3::new(0.0, 0.1, 6.0);
        let mut seen = Vec::new();
        for _ in 0..20 {
            let orders = director.tick(0.5, player, Vec3::new(1.0, 0.0, 0.0));
            for order in &orders {
                if let HordeOrder::Spawn { id, record_id, archetype_id, .. } = order {
                    seen.push((record_id.clone(), archetype_id.clone()));
                    let id = *id;
                    director.notify_gone(id);
                }
            }
        }
        seen
    };
    let a = play(7);
    let b = play(7);
    assert_eq!(a, b);
    assert!(a.len() > 4, "{a:?}");
    let ids: std::collections::BTreeSet<_> = a.iter().map(|(id, _)| id.clone()).collect();
    assert!(ids.len() > 1, "{a:?}");
    let arch: std::collections::BTreeSet<_> = a.iter().map(|(_, id)| id.clone()).collect();
    assert!(arch.len() > 1, "{a:?}");
    assert!(a.iter().all(|(_, id)| fb().contains(id)));
    assert_ne!(a, play(8));
}

#[test]
fn hidden_points_are_preferred() {
    let layout = two_point_layout();
    let mut director = HordeDirector::new(&layout);
    let orders = director.tick(0.2, Vec3::new(0.0, 0.1, 6.0), Vec3::new(0.0, 0.0, 1.0));
    match &orders[0] {
        HordeOrder::Spawn { record_id, .. } => assert_eq!(record_id, "Behind"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn fallback_file_uses_the_same_record_path() {
    let text = fallback_text();
    assert!(!text.contains("70472"), "stand-in must not copy the NH_04 street origin");
    assert!(!text.contains("809.499"));
    assert!(!text.contains("-311"));
    assert!(!text.contains("964"));
    assert!(text.contains("INVENTED_STAND_IN"));
    let raw: Vec<Value> = serde_json::from_str(&text).unwrap();
    let layout = parse(&text);
    assert!(layout.stand_in);
    assert_eq!(layout.budget, Budget::default());
    assert!(layout.records.iter().all(|r| !r.spawner_type.contains("Component")));
    assert!(layout.records.iter().all(|r| r.archetype_id.is_empty() || fb().contains(&r.archetype_id)));
    assert!(layout.tables.values().flatten().all(|(id, _)| fb().contains(id)));
    assert!(!layout.tables.values().flatten().any(|(id, _)| id == "NotARealZombie"));

    let north = by_id(&layout, "Point_North_A");
    assert_eq!(north.archetype_id, "Walker_Burning");
    assert_eq!(north.spawn_table, "Table_NorthLot");
    assert!(north.table_resolved);
    assert!(!north.unknown_type);
    let root = &raw[index_of(&raw.iter().find(|e| e["Name"] == "Point_North_A").unwrap()["Properties"]["RootComponent"])];
    assert!(root["Properties"].get("AttachParent").is_none());
    assert_eq!(north.transform.location_cm, vec3_of(&root["Properties"]["RelativeLocation"]));

    let offset = by_id(&layout, "Point_Offset");
    let child = &raw[index_of(&raw.iter().find(|e| e["Name"] == "Point_Offset").unwrap()["Properties"]["RootComponent"])];
    let parent = &raw[index_of(&child["Properties"]["AttachParent"])];
    let child_loc = vec3_of(&child["Properties"]["RelativeLocation"]);
    let parent_loc = vec3_of(&parent["Properties"]["RelativeLocation"]);
    assert_ne!(offset.transform.location_cm, child_loc);
    assert_eq!(offset.transform.location_cm[0], parent_loc[0] + child_loc[0]);
    assert_eq!(offset.transform.location_cm[1], parent_loc[1] + child_loc[1]);
    assert_eq!(offset.transform.location_cm[2], parent_loc[2] + child_loc[2]);
    assert_eq!(offset.transform.rotation_deg, [0.0, 90.0, 0.0]);
    let (origin, yaw) = layout.frame();
    assert_eq!(origin, [0.0; 3]);
    assert_eq!(yaw, 0.0);
    let g = north.transform.to_godot(origin, yaw);
    assert!((g.x - 12.0).abs() < 1e-3 && (g.y - 0.4).abs() < 1e-3 && (g.z + 4.0).abs() < 1e-3, "{g:?}");

    let south = by_id(&layout, "Point_South_B");
    assert!(south.archetype_id.is_empty());
    assert!(south.fallback_fields.iter().any(|f| f == "Archetype"));
    assert!(south.fallback_fields.iter().any(|f| f.ends_with("RelativeRotation")));
    assert!(south.table_resolved);
    assert_eq!(south.spawn_table, "Table_SouthLot");

    let filled = by_id(&layout, "Point_Filled");
    assert_eq!(filled.spawn_table, "Table_NorthLot");
    assert!(filled.table_resolved);
    assert!(!filled.fallback_fields.iter().any(|f| f == "SpawnTable"));

    let missing = by_id(&layout, "Point_DefaultLoc");
    assert_eq!(missing.transform.location_cm, [0.0; 3]);
    assert!(missing.fallback_fields.iter().any(|f| f.ends_with("RelativeLocation")));
    assert!(missing.fallback_fields.iter().any(|f| f.ends_with("RelativeRotation")));
    assert_eq!(missing.archetype_id, "Walker_Radio");

    let marker = by_id(&layout, "INVENTED_STAND_IN");
    assert!(marker.unknown_type);
    let made = by_id(&layout, "Marker_Unused");
    assert!(made.unknown_type);
    assert_eq!(made.spawner_type, "BP_InventedMarker_C");
    assert_eq!(made.archetype_id, "Walker_Corrosive");

    let player = Vec3::new(0.0, 0.1, 6.0);
    let in_band = layout.records.iter().filter(|r| {
        !r.unknown_type && (r.spawner_type == "BP_ZombieSpawnPoint_C" || r.spawner_type == "BP_Activity_Feasting_C")
    }).filter(|r| {
        let p = r.transform.to_godot(origin, yaw);
        let d = Vec3::new(p.x - player.x, 0.0, p.z - player.z).length();
        d >= 8.0 && d <= 42.0
    }).count();
    assert!(in_band >= 8, "in band {in_band}");

    for loc in layout.records.iter().map(|r| r.transform.location_cm) {
        let far = (loc[0] - NH04_ORIGIN_CM[0]).abs()
            + (loc[1] - NH04_ORIGIN_CM[1]).abs()
            + (loc[2] - NH04_ORIGIN_CM[2]).abs();
        assert!(far > 1000.0, "stand-in location sits on the real map origin: {loc:?}");
    }
}

#[test]
fn load_nh04_uses_the_stand_in_when_no_extract_is_present() {
    let root = repo_root();
    if root.join(gore_core::horde::NH04_EXTRACT_DIR).is_dir() || root.join(gore_core::horde::NH04_SPAWNING_DUMP).is_file() {
        return;
    }
    let layout = load_nh04_with(&root, &fb()).unwrap();
    assert!(layout.stand_in);
    assert!(layout.records.iter().any(|r| r.id == "Point_North_A"));
}

#[test]
fn handwritten_exports_flag_unknowns_and_only_follow_attach_parent() {
    let layout = parse(&json!([
        {"Type":"BP_ZombieSpawnPoint_C","Name":"Alone","Outer":{},"Properties":{
            "RootComponent":{"ObjectPath":"P.1"}, "Archetype":"Walker_Burning"
        }},
        {"Type":"SceneComponent","Name":"AloneRoot","Outer":{},"Properties":{
            "RelativeLocation":{"X":1000.0,"Y":0.0,"Z":0.0},
            "RelativeRotation":{"Pitch":0.0,"Yaw":0.0,"Roll":0.0}
        }},
        {"Type":"BP_ZombieSpawnPoint_C","Name":"Child","Outer":{},"Properties":{
            "RootComponent":{"ObjectPath":"P.3"}, "Archetype":"Runner_Burning",
            "SpawnTable":{"ObjectPath":"P.6"}
        }},
        {"Type":"SceneComponent","Name":"ChildRoot","Outer":{},"Properties":{
            "RelativeLocation":{"X":0.0,"Y":500.0,"Z":0.0},
            "RelativeRotation":{"Pitch":1.0,"Yaw":2.0,"Roll":3.0},
            "AttachParent":{"ObjectPath":"P.4"}
        }},
        {"Type":"SceneComponent","Name":"ParentRoot","Outer":{},"Properties":{
            "RelativeLocation":{"X":1000.0,"Y":0.0,"Z":0.0},
            "RelativeRotation":{"Pitch":0.0,"Yaw":0.0,"Roll":0.0}
        }},
        {"Type":"BP_MadeUpSpawner_C","Name":"MadeUp","Outer":{},"Properties":{
            "RootComponent":{"ObjectPath":"P.5"}
        }},
        {"Type":"BP_Horde_TSL_ZombieCluster_C","Name":"Table_Lot","Outer":{},"Properties":{
            "ArchetypeWeights":[{"Key":"Walker_Burning","Value":1},{"Key":"NotARealZombie","Value":4}],
            "RootComponent":{"ObjectPath":"P.7"},
            "SpawnPoints":[{"ObjectPath":"P.8"}]
        }},
        {"Type":"SceneComponent","Name":"TableRoot","Outer":{},"Properties":{
            "RelativeLocation":{"X":0,"Y":0,"Z":0},"RelativeRotation":{"Pitch":0,"Yaw":0,"Roll":0}
        }},
        {"Type":"BP_ZombieSpawnPoint_C","Name":"Linked","Outer":{},"Properties":{
            "RootComponent":{"ObjectPath":"P.9"},
            "Archetype":"NotARealZombie"
        }},
        {"Type":"SceneComponent","Name":"LinkedRoot","Outer":{},"Properties":{
            "RelativeLocation":{"X":10.0,"Z":4.0},
            "RelativeRotation":{"Pitch":0,"Yaw":0,"Roll":0}
        }},
        {"Type":"BP_ZombieSpawnPoint_C","Name":"Turned","Outer":{},"Properties":{
            "RootComponent":{"ObjectPath":"P.11"}, "Archetype":"Walker_Caustic"
        }},
        {"Type":"SceneComponent","Name":"TurnedRoot","Outer":{},"Properties":{
            "RelativeLocation":{"X":100.0,"Y":0.0,"Z":0.0},
            "RelativeRotation":{"Pitch":0,"Yaw":0,"Roll":0},
            "AttachParent":{"ObjectPath":"P.12"}
        }},
        {"Type":"SceneComponent","Name":"TurnedParent","Outer":{},"Properties":{
            "RelativeLocation":{"X":0,"Y":0,"Z":0},
            "RelativeRotation":{"Pitch":0,"Yaw":90,"Roll":0}
        }}
    ]).to_string());

    let alone = by_id(&layout, "Alone");
    assert_eq!(alone.transform.location_cm, [1000.0, 0.0, 0.0]);
    assert!(alone.fallback_fields.iter().any(|f| f == "SpawnTable"));

    let child = by_id(&layout, "Child");
    assert_eq!(child.transform.location_cm, [1000.0, 500.0, 0.0]);
    assert_eq!(child.transform.rotation_deg, [1.0, 2.0, 3.0]);
    assert_eq!(child.spawn_table, "Table_Lot");
    assert!(child.table_resolved);

    let made = by_id(&layout, "MadeUp");
    assert!(made.unknown_type);
    assert!(layout.records.iter().all(|r| r.spawner_type != "SceneComponent"));

    let weights = &layout.tables["Table_Lot"];
    assert_eq!(weights, &vec![("Walker_Burning".to_string(), 1.0)]);

    let linked = by_id(&layout, "Linked");
    assert!(linked.archetype_id.is_empty());
    assert_eq!(linked.archetype_raw, "NotARealZombie");
    assert_eq!(linked.spawn_table, "Table_Lot");
    assert!(linked.table_resolved);
    assert!(linked.fallback_fields.iter().any(|f| f.ends_with("RelativeLocation.Y")));
    assert_eq!(linked.transform.location_cm, [10.0, 0.0, 4.0]);

    let turned = by_id(&layout, "Turned");
    assert!((turned.transform.location_cm[0]).abs() < 1e-6, "{:?}", turned.transform.location_cm);
    assert!((turned.transform.location_cm[1] - 100.0).abs() < 1e-6, "{:?}", turned.transform.location_cm);
    assert_eq!(turned.transform.rotation_deg, [0.0, 0.0, 0.0]);
}

#[test]
fn report_scans_every_tile_and_writes_nothing() {
    let dir = std::env::temp_dir().join(format!("horde-report-{}-{}", std::process::id(), line!()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("tiles")).unwrap();
    fs::write(dir.join("tiles/NH_04_T01_Fixed.json"), point_file("FromFixed", Some("Walker_Burning"), [100.0, 0.0, 0.0])).unwrap();
    fs::write(dir.join("tiles/NH_04_T01_Dynamic.json"), point_file("FromDynamic", None, [200.0, 0.0, 0.0])).unwrap();
    fs::write(dir.join("tiles/NH_04_T01_Lighting.json"), json!([
        {"Type":"BP_InventedLamp_C","Name":"FromLighting","Outer":{},"Properties":{
            "RootComponent":{"ObjectPath":"L.1"}
        }},
        {"Type":"SceneComponent","Name":"LampRoot","Outer":{},"Properties":{
            "RelativeLocation":{"X":1,"Y":2,"Z":3},
            "RelativeRotation":{"Pitch":0,"Yaw":0,"Roll":0}
        }}
    ]).to_string()).unwrap();
    fs::write(dir.join("notes.json"), "{\"not\":\"an array\"}").unwrap();
    let before = snapshot(&dir);

    let scan = scan_dir_with(&dir, &fb()).unwrap();
    assert_eq!(snapshot(&dir), before, "scan_dir must not write");
    let report = format_report(&scan.layout, &scan.files_scanned, &scan.files_skipped);
    assert!(report.starts_with(REPORT_HEADER));
    assert!(report.contains("files scanned: 3"));
    assert!(report.contains("tiles/NH_04_T01_Fixed.json"));
    assert!(report.contains("tiles/NH_04_T01_Dynamic.json"));
    assert!(report.contains("tiles/NH_04_T01_Lighting.json"));
    assert!(report.contains("files skipped: 1"));
    assert!(report.contains("notes.json"));
    assert!(report.contains("spawners found: 3"));
    assert!(report.contains("FromFixed | BP_ZombieSpawnPoint_C | tiles/NH_04_T01_Fixed.json"));
    assert!(report.contains("FromDynamic | BP_ZombieSpawnPoint_C | tiles/NH_04_T01_Dynamic.json"));
    assert!(report.contains("FromLighting | BP_InventedLamp_C | tiles/NH_04_T01_Lighting.json"));
    assert!(report.contains("spawners of unknown type: 1"));
    assert!(report.contains("FromLighting | BP_InventedLamp_C"));
    assert!(report.contains("spawners with no matching archetype id:"));
    assert!(report.contains("FromDynamic |"));
    assert!(report.contains("fields filled from fallbacks:"));
    assert!(report.contains("FromDynamic | Archetype"));
    assert!(report.contains("spawn tables resolved:"));

    let bin = env!("CARGO_BIN_EXE_horde_report");
    // Point the binary at a table that does not exist so it uses the same fallback set as `fb()`.
    let out = Command::new(bin)
        .arg(&dir)
        .current_dir(&dir)
        .env("DI2_ARCHETYPES_JSON", dir.join("no-table.json"))
        .output()
        .expect("run horde-report");
    assert!(out.status.success(), "stderr {}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8(out.stdout).unwrap(), report);
    assert_eq!(snapshot(&dir), before, "horde-report must not write");
    let _ = fs::remove_dir_all(&dir);
}

fn spawn_count(orders: &[HordeOrder]) -> usize {
    orders.iter().filter(|o| matches!(o, HordeOrder::Spawn { .. })).count()
}

fn point_file(name: &str, archetype: Option<&str>, loc: [f64; 3]) -> String {
    let mut props = json!({"RootComponent": {"ObjectPath": "F.1"}});
    if let Some(id) = archetype {
        props["Archetype"] = json!(id);
    }
    json!([
        {"Type":"BP_ZombieSpawnPoint_C","Name":name,"Outer":{},"Properties":props},
        {"Type":"SceneComponent","Name":format!("{name}Root"),"Outer":{},"Properties":{
            "RelativeLocation":{"X":loc[0],"Y":loc[1],"Z":loc[2]},
            "RelativeRotation":{"Pitch":0,"Yaw":0,"Roll":0}
        }}
    ]).to_string()
}

fn ring_layout(budget: Value) -> SpawnLayout {
    let mut exports = vec![
        json!({"Type":"StandInLayout_C","Name":"INVENTED_STAND_IN","Outer":{},"Properties":{"StandIn":true,"RootComponent":{"ObjectPath":"T.1"}}}),
        json!({"Type":"SceneComponent","Name":"MarkerRoot","Outer":{},"Properties":{
            "RelativeLocation":{"X":0,"Y":0,"Z":0},"RelativeRotation":{"Pitch":0,"Yaw":0,"Roll":0}
        }}),
        json!({"Type":"HordeDirectorBudget","Name":"Budget","Outer":{},"Properties":budget}),
    ];
    // Godot (local frame): (12, 0.4, -4), (-16, 0.4, 2), (8, 0.4, 22), (28, 0.4, 10)
    for (name, arch, loc) in [
        ("A", "Walker_Burning", [1200.0, -400.0, 40.0]),
        ("B", "Runner_Burning", [-1600.0, 200.0, 40.0]),
        ("C", "Walker_Fireman", [800.0, 2200.0, 40.0]),
        ("D", "Runner_Fireman", [2800.0, 1000.0, 40.0]),
    ] {
        push_point(&mut exports, name, arch, loc);
    }
    parse(&Value::Array(exports).to_string())
}

fn weighted_layout(cooldown: f32) -> SpawnLayout {
    let mut exports = vec![
        json!({"Type":"StandInLayout_C","Name":"INVENTED_STAND_IN","Outer":{},"Properties":{"StandIn":true,"RootComponent":{"ObjectPath":"T.1"}}}),
        json!({"Type":"SceneComponent","Name":"MarkerRoot","Outer":{},"Properties":{
            "RelativeLocation":{"X":0,"Y":0,"Z":0},"RelativeRotation":{"Pitch":0,"Yaw":0,"Roll":0}
        }}),
        json!({"Type":"HordeDirectorBudget","Name":"Budget","Outer":{},"Properties":{
            "MaxActive":1,"MinSpawnDistance":8,"MaxSpawnDistance":42,"DespawnDistance":80,
            "SpawnInterval":0,"MaxSpawnsPerTick":1,"PointCooldown":cooldown,"ViewHalfAngleDeg":180,"Seed":7
        }}),
        json!({"Type":"BP_Horde_TSL_ZombieCluster_C","Name":"Table_Mix","Outer":{},"Properties":{
            "ArchetypeWeights":[{"Key":"Walker_Shocking","Value":1},{"Key":"Runner_Shocking","Value":3},{"Key":"NotARealZombie","Value":9}],
            "RootComponent":{"ObjectPath":"T.4"}
        }}),
        json!({"Type":"SceneComponent","Name":"MixRoot","Outer":{},"Properties":{
            "RelativeLocation":{"X":0,"Y":0,"Z":0},"RelativeRotation":{"Pitch":0,"Yaw":0,"Roll":0}
        }}),
    ];
    for (name, loc) in [
        ("Near", [1200.0, -400.0, 40.0]),
        ("Mid", [800.0, 2200.0, 40.0]),
        ("Far", [2800.0, 1000.0, 40.0]),
        ("Side", [-2200.0, -800.0, 40.0]),
    ] {
        let actor = exports.len();
        exports.push(json!({"Type":"BP_ZombieSpawnPoint_C","Name":name,"Outer":{},"Properties":{"SpawnTable":"Table_Mix"}}));
        let root = exports.len();
        exports.push(json!({"Type":"SceneComponent","Name":format!("{name}Root"),"Outer":{},"Properties":{
            "RelativeLocation":{"X":loc[0],"Y":loc[1],"Z":loc[2]},
            "RelativeRotation":{"Pitch":0,"Yaw":0,"Roll":0}
        }}));
        exports[actor]["Properties"]["RootComponent"] = json!({"ObjectPath": format!("T.{root}")});
    }
    parse(&Value::Array(exports).to_string())
}

fn two_point_layout() -> SpawnLayout {
    let mut exports = vec![
        json!({"Type":"StandInLayout_C","Name":"INVENTED_STAND_IN","Outer":{},"Properties":{"StandIn":true,"RootComponent":{"ObjectPath":"T.1"}}}),
        json!({"Type":"SceneComponent","Name":"MarkerRoot","Outer":{},"Properties":{
            "RelativeLocation":{"X":0,"Y":0,"Z":0},"RelativeRotation":{"Pitch":0,"Yaw":0,"Roll":0}
        }}),
        json!({"Type":"HordeDirectorBudget","Name":"Budget","Outer":{},"Properties":{
            "MaxActive":1,"MinSpawnDistance":8,"MaxSpawnDistance":42,"DespawnDistance":64,
            "SpawnInterval":0,"MaxSpawnsPerTick":1,"PointCooldown":0,"ViewHalfAngleDeg":55,"Seed":1
        }}),
    ];
    push_point(&mut exports, "InFront", "Walker_Burning", [0.0, 2600.0, 40.0]);
    push_point(&mut exports, "Behind", "Runner_Burning", [0.0, -1400.0, 40.0]);
    parse(&Value::Array(exports).to_string())
}

fn push_point(exports: &mut Vec<Value>, name: &str, archetype: &str, loc: [f64; 3]) {
    let actor = exports.len();
    exports.push(json!({
        "Type":"BP_ZombieSpawnPoint_C","Name":name,"Outer":{},
        "Properties":{"Archetype": archetype}
    }));
    let root = exports.len();
    exports.push(json!({
        "Type":"SceneComponent","Name":format!("{name}Root"),"Outer":{},
        "Properties":{
            "RelativeLocation":{"X":loc[0],"Y":loc[1],"Z":loc[2]},
            "RelativeRotation":{"Pitch":0,"Yaw":0,"Roll":0}
        }
    }));
    exports[actor]["Properties"]["RootComponent"] = json!({"ObjectPath": format!("T.{root}")});
}

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for ent in fs::read_dir(dir).unwrap() {
        let path = ent.unwrap().path();
        if path.is_dir() {
            walk(&path, root, out);
        } else {
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            out.push((rel, fs::read(&path).unwrap()));
        }
    }
}

#[test]
fn stand_in_borrows_ids_from_a_loaded_table() {
    let table = fixture_table();
    let order = table.seeded_order(gore_core::horde::STAND_IN_ID_SEED);
    assert_eq!(order, vec!["Invented_Bravo", "Invented_Charlie", "Invented_Alpha"], "sorted, rotated by seed 1");
    let map = stand_in_id_map(&table);
    assert_eq!(map.len(), FALLBACK_ARCHETYPE_IDS.len());
    for (i, id) in FALLBACK_ARCHETYPE_IDS.iter().enumerate() {
        assert_eq!(map[*id], order[i % order.len()], "{id}");
    }

    let layout = parse_layout_str_with(&fallback_text(), &table).unwrap();
    assert!(layout.stand_in);
    let north = by_id(&layout, "Point_North_A");
    assert_eq!(north.archetype_raw, "Walker_Burning", "the stand-in text is kept");
    assert_eq!(north.archetype_id, map["Walker_Burning"]);
    assert!(layout.records.iter().all(|r| r.archetype_id.is_empty() || table.contains(&r.archetype_id)));
    assert!(layout.tables.values().flatten().all(|(id, _)| table.contains(id)));
    assert!(!layout.tables.is_empty(), "stand-in weights follow the table too");

    let spawned = spawned_ids(&layout, 40);
    assert!(!spawned.is_empty(), "a table with rows must not give 0 spawns");
    assert!(spawned.iter().all(|id| table.contains(id)), "{spawned:?}");
    assert_eq!(spawned, spawned_ids(&layout, 40), "fixed order, fixed seed");

    // The same stand-in against the fallback set keeps its own ids.
    let plain = parse(&fallback_text());
    assert_eq!(by_id(&plain, "Point_North_A").archetype_id, "Walker_Burning");
    assert!(!spawned_ids(&plain, 40).is_empty());
}

#[test]
fn a_real_dump_is_never_remapped() {
    // No stand-in marker: ids outside the loaded table stay unmatched, as the report counts them.
    let text = point_file("RealPoint", Some("Walker_Burning"), [1200.0, -400.0, 40.0]);
    let layout = parse_layout_str_with(&text, &fixture_table()).unwrap();
    assert!(!layout.stand_in);
    let rec = by_id(&layout, "RealPoint");
    assert!(rec.archetype_id.is_empty());
    assert_eq!(rec.archetype_raw, "Walker_Burning");
    let report = format_report(&layout, &[], &[]);
    assert!(report.contains("archetype ids: 3 from fixture/archetypes.json"), "{report}");
    assert!(report.contains("spawners with no matching archetype id: 1"), "{report}");
}

/// Uses the real, locally generated table (gitignored). Ignored without `--features game-data`.
#[test]
#[cfg_attr(not(feature = "game-data"), ignore = "needs game data")]
fn real_table_stand_in_spawns_only_table_ids() {
    let path = ArchetypeIds::default_path();
    let table = ArchetypeIds::from_path(&path);
    assert!(table.is_table(), "needs game data: {} ({})", path.display(), table.describe());
    let layout = parse_layout_str_with(&fallback_text(), &table).unwrap();
    let spawned = spawned_ids(&layout, 40);
    let bad: Vec<&String> = spawned.iter().filter(|id| !table.contains(id)).collect();
    let distinct: std::collections::BTreeSet<&String> = spawned.iter().collect();
    eprintln!(
        "real table: {} | spawns: {} | distinct ids: {} | ids not in table: {}",
        table.describe(),
        spawned.len(),
        distinct.len(),
        bad.len()
    );
    assert!(!spawned.is_empty(), "0 spawns with a loaded table");
    assert!(bad.is_empty(), "{bad:?}");
}
