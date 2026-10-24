//! Every archetype converted from the local DI2 data must parse, and its survival rule must compile.
//! Needs the gitignored `game/data/archetypes.json` (tools/convert_archetypes.py) or `DI2_ARCHETYPES_JSON`,
//! so it is ignored unless the `game-data` feature is on. With the feature a missing table fails.
use gore_core::horde::ArchetypeIds;
use gore_core::Archetype;

#[test]
#[cfg_attr(not(feature = "game-data"), ignore = "needs game data")]
fn converted_archetypes_load() {
    // Same resolution as the horde director: DI2_ARCHETYPES_JSON, else <repo>/game/data/archetypes.json.
    let path = ArchetypeIds::default_path();
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "needs game data: {} ({e}). Generate it with tools/convert_archetypes.py or set DI2_ARCHETYPES_JSON.",
            path.display()
        )
    });
    let all: Vec<Archetype> = serde_json::from_str(&src).expect("archetypes.json parses");
    assert!(all.iter().any(|a| a.name == "Walker"));
    for a in &all {
        a.survive_expr().unwrap_or_else(|e| panic!("{}: {e}", a.name));
        assert!(!a.parts.is_empty(), "{} has parts", a.name);
    }
}
