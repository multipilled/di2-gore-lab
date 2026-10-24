//! Ticket 2 spawn read: archetype chains, 0-based export indices, vegetation skip and attribute values.
//!
//! Fixtures are small invented export arrays written to a temp `Content` root. No value, name or
//! number here comes from Dead Island 2. The one test that reads the player's export is gated behind
//! the `game-data` feature.

use std::fs;
use std::path::{Path, PathBuf};

use gore_core::horde::{format_read_report, format_report, scan_dir_with, ArchetypeIdSource, ArchetypeIds};
use gore_core::spawn_read::{
    archetype_chain, is_vegetation_level, resolve_attributes, split_object_path, AttrSource, ChainEnd, Packages,
    NATIVE_DEFAULT,
};
use serde_json::{json, Value};

fn ids() -> ArchetypeIds {
    ArchetypeIds {
        ids: ["Invented", "Invented_Hot"].iter().map(|s| s.to_string()).collect(),
        source: ArchetypeIdSource::Table(PathBuf::from("fixture/archetypes.json")),
    }
}

fn temp_root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("spawn-read-{}-{tag}", std::process::id())).join("Content");
    let _ = fs::remove_dir_all(d.parent().unwrap());
    fs::create_dir_all(&d).unwrap();
    d
}

fn write(root: &Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, v.to_string()).unwrap();
}

fn r(path: &str) -> Value {
    json!({"ObjectName": "x", "ObjectPath": path})
}

const LVL: &str = "/Game/Inv/Maps/M/M_Spawning";

/// Level: 0 point, 1 point root, 2 cluster, 3 cluster root, 4 agent source, 5 activity, 6 slot, 7 level,
/// 8 activity root, 9 slot with AgentSource only.
fn level() -> Value {
    json!([
        {"Type":"BP_ZombieSpawnPoint_C","Name":"Point0","Outer":r(&format!("{LVL}.7")),
         "Properties":{"RootComponent":r(&format!("{LVL}.1")),"Cluster":r(&format!("{LVL}.2"))}},
        {"Type":"SceneComponent","Name":"PointRoot","Outer":r(&format!("{LVL}.0")),"Properties":{
            "AttachParent":r(&format!("{LVL}.3")),
            "RelativeLocation":{"X":10.0,"Y":0.0,"Z":0.0},"RelativeRotation":{"Pitch":0.0,"Yaw":0.0,"Roll":0.0}}},
        {"Type":"BP_Horde_TSL_ZombieCluster_C","Name":"Cluster0","Outer":r(&format!("{LVL}.7")),
         "Properties":{"RootComponent":r(&format!("{LVL}.3")),"AgentSource":r(&format!("{LVL}.4"))}},
        {"Type":"SceneComponent","Name":"ClusterRoot","Outer":r(&format!("{LVL}.2")),"Properties":{
            "RelativeLocation":{"X":100.0,"Y":200.0,"Z":0.0},"RelativeRotation":{"Pitch":0.0,"Yaw":0.0,"Roll":0.0}}},
        {"Type":"DefaultAgentSource","Name":"Agent0","Outer":r(&format!("{LVL}.2")),
         "Template":r("/Game/Inv/TSL/BP_Cluster.2")},
        {"Type":"BP_SpawnActivity_Invented_C","Name":"Activity0","Outer":r(&format!("{LVL}.7")),
         "Properties":{"RootComponent":r(&format!("{LVL}.8")),"ParticipantSlot":r(&format!("{LVL}.6"))}},
        {"Type":"ParticipantSlotComponent","Name":"ParticipantSlot","Outer":r(&format!("{LVL}.5")),
         "Properties":{"Participant":{"SpawnableGroup":r("/Game/Inv/Groups/DA_Group_Missing.0")}}},
        {"Type":"Level","Name":"PersistentLevel","Outer":r(&format!("{LVL}.10"))},
        {"Type":"SceneComponent","Name":"ActivityRoot","Outer":r(&format!("{LVL}.5")),"Properties":{
            "RelativeLocation":{"X":1.0,"Y":2.0,"Z":3.0},"RelativeRotation":{"Pitch":0.0,"Yaw":0.0,"Roll":0.0}}},
        {"Type":"ParticipantSlotComponent","Name":"SlotWithSource","Outer":r(&format!("{LVL}.5")),
         "Properties":{"AgentSource":r(&format!("{LVL}.4"))}}
    ])
}

#[test]
fn object_paths_are_zero_based_export_indices() {
    assert_eq!(split_object_path("/Game/A/B.0"), ("/Game/A/B", Some(0)));
    assert_eq!(split_object_path("/Game/A/B.12"), ("/Game/A/B", Some(12)));
    assert_eq!(split_object_path("/Script/Engine"), ("/Script/Engine", None));
    let root = temp_root("index");
    // Export 2 of the class package is the one the agent source's Template names; 0 and 1 are decoys.
    write(&root, "Inv/TSL/BP_Cluster.json", &json!([
        {"Type":"X","Name":"Decoy0","Properties":{"Archetype":"Wrong0"}},
        {"Type":"X","Name":"Decoy1","Properties":{"Archetype":"Wrong1"}},
        {"Type":"DefaultAgentSource","Name":"Default__Agent","Properties":{"Archetype":"Invented"}}
    ]));
    write(&root, "Inv/Maps/M/M_Spawning.json", &level());
    let exports: Vec<Value> = level().as_array().unwrap().clone();
    let mut pk = Packages::new(Some(root.clone()));
    let chain = archetype_chain(&exports, 0, &mut pk);
    assert_eq!(chain.end, ChainEnd::Archetype("Invented".into()), "{}", chain.describe());
    assert!(chain.describe().starts_with("Cluster Cluster0 -> DefaultAgentSource Agent0 -> class default /Game/Inv/TSL/BP_Cluster.2"));
    let _ = fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn chains_stop_at_the_first_link_missing_from_the_export() {
    let root = temp_root("missing");
    let exports: Vec<Value> = level().as_array().unwrap().clone();
    let mut pk = Packages::new(Some(root.clone()));
    let point = archetype_chain(&exports, 0, &mut pk);
    assert_eq!(
        point.end,
        ChainEnd::NotDumped { what: "class default", package: "/Game/Inv/TSL/BP_Cluster".into(), export: Some(2) }
    );
    let activity = archetype_chain(&exports, 5, &mut pk);
    assert_eq!(
        activity.end,
        ChainEnd::NotDumped { what: "spawnable group", package: "/Game/Inv/Groups/DA_Group_Missing".into(), export: Some(0) }
    );
    assert!(activity.describe().contains("ParticipantSlotComponent ParticipantSlot -> SpawnableGroup /Game/Inv/Groups/DA_Group_Missing"));
    // A dumped group whose archetype field this reader does not know is reported, not guessed.
    write(&root, "Inv/Groups/DA_Group_Missing.json", &json!([{"Type":"G","Name":"DA_Group_Missing","Properties":{"Unknown":1}}]));
    let mut pk = Packages::new(Some(root.clone()));
    let activity = archetype_chain(&exports, 5, &mut pk);
    assert_eq!(activity.end, ChainEnd::NotRead { what: "spawnable group", package: "/Game/Inv/Groups/DA_Group_Missing".into() });
    let _ = fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn class_default_chain_climbs_templates_and_native_ends_say_so() {
    let root = temp_root("climb");
    write(&root, "Inv/TSL/BP_Cluster.json", &json!([
        {"Type":"X","Name":"A"}, {"Type":"X","Name":"B"},
        {"Type":"DefaultAgentSource","Name":"Default__Agent","Template":r("/Game/Inv/TSL/BP_ClusterBase.0")}
    ]));
    write(&root, "Inv/TSL/BP_ClusterBase.json", &json!([
        {"Type":"DefaultAgentSource","Name":"Default__Base","Template":r("/Script/Invented.Default__DefaultAgentSource")}
    ]));
    let exports: Vec<Value> = level().as_array().unwrap().clone();
    let mut pk = Packages::new(Some(root.clone()));
    let chain = archetype_chain(&exports, 0, &mut pk);
    assert!(matches!(chain.end, ChainEnd::Native(_)), "{}", chain.describe());
    assert!(chain.end_text().starts_with(NATIVE_DEFAULT));
    assert!(chain.describe().contains("class default /Game/Inv/TSL/BP_ClusterBase.0"));
    let _ = fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn vegetation_levels_are_skipped_and_the_read_report_lists_chains() {
    assert!(is_vegetation_level("SubLevels/M_Vegetation.json"));
    assert!(!is_vegetation_level("SubLevels/M_T01_Dynamic.json"));
    let root = temp_root("scan");
    let dir = root.join("Inv/Maps/M");
    write(&root, "Inv/Maps/M/M_Spawning.json", &level());
    write(&root, "Inv/Maps/M/SubLevels/M_Vegetation.json", &level());
    let scan = scan_dir_with(&dir, &ids()).unwrap();
    assert_eq!(scan.files_scanned, vec!["M_Spawning.json".to_string()]);
    assert_eq!(scan.files_skipped.len(), 1);
    assert!(scan.files_skipped[0].1.contains("vegetation"));
    let point = scan.layout.records.iter().find(|r| r.id == "Point0").unwrap();
    assert_eq!(point.transform.location_cm, [110.0, 200.0, 0.0]);
    assert_eq!(point.spawn_table, "Cluster0");
    let act = scan.layout.records.iter().find(|r| r.id == "Activity0").unwrap();
    assert!(act.activity && !act.unknown_type);
    let report = format_read_report(&scan);
    assert!(report.contains("M_Spawning.json | known spawners 3 | BP_ZombieSpawnPoint_C 1 | not a tile"), "{report}");
    assert!(report.contains("UNVERIFIED 115: files with known spawners that are _Dynamic tiles: 0 of 1"), "{report}");
    assert!(report.contains("class default not in export: /Game/Inv/TSL/BP_Cluster (export 2)"), "{report}");
    assert!(report.contains("spawnable group not in export: /Game/Inv/Groups/DA_Group_Missing (export 0)"), "{report}");
    // Five counts still come from format_report.
    let base = format_report(&scan.layout, &scan.files_scanned, &scan.files_skipped);
    assert!(base.contains("files skipped: 1"));
    assert!(base.contains("spawners found: 3"));
    let _ = fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn attribute_values_follow_variant_then_class_then_type_default() {
    let root = temp_root("attrs");
    let ty = |n: &str| r(&format!("/Game/Inv/Types/DA_AttributeType_{n}.0"));
    write(&root, "Inv/Balance/DA_AIAttributeValues_Invented.json", &json!([{"Type":"S","Name":"c","Properties":{
        "Own": {"AttributeType": ty("Own"), "InitialSettings": {"bUseTypeDefaultValue": false, "DesiredValue": 7.0}},
        "Overridden": {"AttributeType": ty("Over"), "InitialSettings": {"bUseTypeDefaultValue": false, "DesiredValue": 1.0}},
        "FromType": {"AttributeType": ty("Known")},
        "TypeMissing": {"AttributeType": ty("Missing"), "InitialSettings": {"bUseDefaultLevelScalingSettings": false}},
        "TypeNoDefault": {"AttributeType": ty("Empty")},
        "HalfRange": {"AttributeType": ty("Own"), "InitialSettings": {"bUseTypeDefaultValue": false, "MaxValue": 4.0}},
        "Nothing": {"AttributeType": ty("Own"), "InitialSettings": {"bUseTypeDefaultValue": false}},
        "TypeKey": {"Type": ty("Known")},
        "Extra": [{"Type": ty("Known"), "InitialSettings": {"bUseTypeDefaultValue": true, "DesiredValue": 9.0}}]
    }}]));
    write(&root, "Inv/Balance/DA_AIAttributeValues_Invented_Hot.json", &json!([{"Type":"S","Name":"v","Properties":{
        "Overridden": {"AttributeType": ty("Over"), "InitialSettings": {"bUseTypeDefaultValue": false, "DesiredValue": 2.0}}
    }}]));
    write(&root, "Inv/Types/DA_AttributeType_Known.json", &json!([{"Type":"T","Name":"k","Properties":{"DefaultValue": 3.5}}]));
    write(&root, "Inv/Types/DA_AttributeType_Empty.json", &json!([{"Type":"T","Name":"e","Properties":{}}]));
    let mut pk = Packages::new(Some(root.clone()));

    let hot = resolve_attributes(&mut pk, "Invented_Hot");
    assert!(hot.class_asset.is_some() && hot.variant_asset.is_some());
    let get = |n: &str| hot.values.iter().find(|v| v.field == n).unwrap().clone();
    assert_eq!((get("Own").desired, get("Own").source), (Some(7.0), AttrSource::Class));
    assert_eq!((get("Overridden").desired, get("Overridden").source), (Some(2.0), AttrSource::Variant));
    assert_eq!(get("FromType").desired, Some(3.5));
    assert_eq!(get("FromType").source, AttrSource::TypeDefault("/Game/Inv/Types/DA_AttributeType_Known".into()));
    assert_eq!(get("TypeMissing").source, AttrSource::TypeAssetNotDumped("/Game/Inv/Types/DA_AttributeType_Missing".into()));
    assert!(matches!(get("TypeNoDefault").source, AttrSource::Native(_)));
    assert!(get("TypeNoDefault").describe().contains(NATIVE_DEFAULT));
    assert_eq!((get("HalfRange").max, get("HalfRange").native_parts.clone()), (Some(4.0), vec!["MinValue"]));
    assert!(get("HalfRange").describe().contains(&format!("range {NATIVE_DEFAULT}..4")));
    assert!(matches!(get("Nothing").source, AttrSource::Native(_)));
    assert_eq!(get("TypeKey").desired, Some(3.5));
    // bUseTypeDefaultValue true: the type's DefaultValue wins over the entry's own DesiredValue.
    let extra = get("Extra[DA_AttributeType_Known]");
    assert_eq!((extra.desired, extra.source), (Some(3.5), AttrSource::TypeDefault("/Game/Inv/Types/DA_AttributeType_Known".into())));

    let base = resolve_attributes(&mut pk, "Invented");
    assert!(base.variant_asset.is_none());
    let over = base.values.iter().find(|v| v.field == "Overridden").unwrap();
    assert_eq!((over.desired, over.source.clone()), (Some(1.0), AttrSource::Class));
    let _ = fs::remove_dir_all(root.parent().unwrap());
}

/// The player's own NH_04 export. `DI2_EXTRACT_ROOT` is the CUE4Parse `Content` root (default
/// `<repo>/extracted/json/DeadIsland/Content`). Ignored without `--features game-data`; fails loudly
/// when the export is missing. Prints the five horde counts; asserts only structure, not DI2 values.
#[test]
#[cfg_attr(not(feature = "game-data"), ignore = "needs game data")]
fn real_nh04_spawn_read() {
    let root = std::env::var_os("DI2_EXTRACT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extracted/json/DeadIsland/Content"));
    let dir = root.join("DI2/Horde/Maps/NH_04");
    assert!(dir.is_dir(), "needs game data: {}", dir.display());
    let table = ArchetypeIds::from_path(&ArchetypeIds::default_path());
    let scan = scan_dir_with(&dir, &table).unwrap();
    let l = &scan.layout;
    let known: Vec<_> = l.records.iter().filter(|r| !r.unknown_type).collect();
    let points = known.iter().filter(|r| r.spawner_type == "BP_ZombieSpawnPoint_C").count();
    let no_arch = l.records.iter().filter(|r| !l.ids.contains(&r.archetype_id)).count();
    let filled: usize = l.records.iter().map(|r| r.fallback_fields.len()).sum();
    let tables = l.records.iter().filter(|r| r.table_resolved && !r.spawn_table.is_empty()).count();
    eprintln!(
        "nh04 read: {} | files scanned {} | files skipped {} | spawners found {} | unknown type {} | no matching archetype id {} | fields filled from fallbacks {} | spawn tables resolved {} | known {} | spawn points {}",
        l.ids.describe(), scan.files_scanned.len(), scan.files_skipped.len(), l.records.len(),
        l.records.iter().filter(|r| r.unknown_type).count(), no_arch, filled, tables, known.len(), points
    );
    assert!(scan.content_root.is_some());
    assert!(scan.files_skipped.iter().any(|(f, why)| f.contains("Vegetation") && why.contains("vegetation")));
    assert!(!scan.files_scanned.iter().any(|f| f.contains("Vegetation")));
    assert!(points > 0, "no BP_ZombieSpawnPoint_C read");
    assert!(known.iter().all(|r| !r.archetype_chain.is_empty()));
    assert!(l.records.iter().filter(|r| r.spawner_type == "BP_ZombieSpawnPoint_C").all(|r| r.table_resolved));
    let report = format_read_report(&scan);
    assert!(report.contains("attribute values:"));
    for line in report.lines().filter(|l| !l.starts_with("  ")) {
        eprintln!("nh04 read: {line}");
    }
}
