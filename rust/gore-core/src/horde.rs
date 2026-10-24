//! Horde population for a map: spawn records and a distance-capped director.
//!
//! A CUE4Parse export list is a flat JSON array. Each object has `Type`, `Name`, `Outer` and
//! `Properties`. The same parser accepts the invented NH_04 stand-in (`game/spawn/nh04_fallback.json`)
//! and a directory of dumps from the user's install. Records are engine-neutral: an id, a UE
//! transform in centimetres, an archetype id, and a spawn-table link. Types that are not one of
//! the known spawner classes are kept and marked `unknown_type`.
//!
//! Allowed archetype ids are the `name` of every row in the locally generated `game/data/archetypes.json`
//! (gitignored; path override `DI2_ARCHETYPES_JSON`), loaded once. When that table is missing the
//! loader falls back to [`FALLBACK_ARCHETYPE_IDS`]. See [`archetype_ids`]. Every entry point also has a
//! `_with` form that takes an explicit [`ArchetypeIds`], so callers (and tests) need not read the
//! on-disk table.
//!
//! The invented stand-in only names fallback ids. When a loaded table is in use, each stand-in id at
//! index `i` of [`FALLBACK_ARCHETYPE_IDS`] is replaced by row `i % len` of the table's
//! [`ArchetypeIds::seeded_order`], so the stand-in still spawns (see [`stand_in_id_map`]).
//!
//! `RelativeLocation` is already in world space unless that component has an `AttachParent`. The
//! parent chain is followed only when `AttachParent` is present. Cooked exports omit fields that
//! still have the class default, and those defaults are not in the JSON: a missing field is counted
//! as fallback-filled (engine zero for a missing transform number) and listed, not treated as a
//! measured Dead Island 2 value.
//!
//! Levels are split into tiles named `<Map>_T<nn>_Fixed`, `_Dynamic` and `_Lighting`. Spawners are
//! suspected to live in the `_Dynamic` tiles. That split is unconfirmed (recon/UNVERIFIED.md 115), so a
//! directory scan reads every JSON file, including `_Fixed` and `_Lighting`. Vegetation levels are the
//! one exception: they are skipped and listed as skipped.
//!
//! When the scanned directory sits under a CUE4Parse `Content` root, a spawner with no archetype field
//! of its own has its archetype followed through its component export and the class default chain
//! ([`crate::spawn_read`]), and the report lists where each chain stopped plus the attribute values
//! of every allowed archetype id.
//!
//! `game/spawn/nh04_fallback.json` is an invented stand-in. It is not extracted from Dead Island 2.
//! Its positions and spawn-table names are made up. Pacing numbers on [`Budget`] are stand-ins too
//! (recon/UNVERIFIED.md).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use glam::Vec3;
use serde_json::Value;

use crate::damage::Rng;
use crate::spawn_read::{self, Packages};

/// Fallback allow-list, used only when `archetypes.json` is not present (a clean checkout, CI).
///
/// It is limited to these 22 because they are the P2 elemental and hazard rows already recorded in
/// recon/ARCHETYPES.md (Q14), which is committed text and not game data. The base Walker / Runner /
/// Shambler / Thug bodies and the specials are left out on purpose: their ids come from the generated
/// table, so a checkout without it cannot vouch for them. With the table present this list is unused.
pub const FALLBACK_ARCHETYPE_IDS: &[&str] = &[
    "Walker_Burning",
    "Runner_Burning",
    "Walker_Fireman",
    "Runner_Fireman",
    "Walker_Shocking",
    "Runner_Shocking",
    "Walker_Caustic",
    "Runner_Caustic",
    "Walker_Spiky",
    "Runner_Spiky",
    "Walker_Vicious",
    "Runner_Vicious",
    "Walker_FuelTank",
    "Walker_WaterTank",
    "Walker_Corrosive",
    "Walker_GrenadeVest",
    "Walker_InsectSwarm",
    "Walker_Radio",
    "Walker_RiotGear_Light",
    "Walker_RiotGear_Medium",
    "Walker_RiotGear_VeryLight",
    "Walker_RiotGear_Heavy",
];

const KNOWN_SPAWNER_TYPES: &[&str] = &[
    "BP_ZombieSpawnPoint_C",
    "BP_Horde_TSL_ZombieCluster_C",
    "BP_Activity_Feasting_C",
];

const POPULATION_TYPES: &[&str] = &["BP_ZombieSpawnPoint_C", "BP_Activity_Feasting_C"];

/// An emergent activity actor (`BP_SpawnActivity_*`, `BP_EntryActivity_*`, ...): it owns a
/// `ParticipantSlot` component that names who spawns there. Counted as a known spawner.
fn is_activity(props: &Value) -> bool {
    props.get("ParticipantSlot").map(|v| !v.is_null()).unwrap_or(false)
}

/// Candidate property names. Which one a cooked spawner uses is unverified; a missing key is the
/// class default and is counted as fallback-filled rather than replaced with a guessed id.
const ARCHETYPE_KEYS: &[&str] = &[
    "Archetype",
    "ArchetypeId",
    "ZombieArchetype",
    "SpawnArchetype",
    "ZombieType",
];

const TABLE_KEYS: &[&str] = &["SpawnTable", "SpawnTableId", "ZombieCluster", "Cluster"];

/// Directory of CUE4Parse JSON for Horde NH_04, relative to the repo root. Gitignored (`/extracted/`).
/// Tile files `<Map>_T<nn>_Fixed`, `_Dynamic` and `_Lighting` live under here, as does `NH_04_Spawning`
/// when that sublevel was exported. The loader scans all of them.
pub const NH04_EXTRACT_DIR: &str = "extracted/json/DeadIsland/Content/DI2/Horde/Maps/NH_04";

/// Single-file hook kept for an export of the spawning sublevel on its own. Gitignored.
pub const NH04_SPAWNING_DUMP: &str =
    "extracted/json/DeadIsland/Content/DI2/Horde/Maps/NH_04/SubLevels/NH_04_Spawning.json";

pub const FALLBACK_REL: &str = "game/spawn/nh04_fallback.json";

/// UE centimetres. Same frame as `--map=nh04` in tools/build_test_map.py. Used only to place a real
/// dump into that map. The stand-in layout does not use this origin.
pub const NH04_ORIGIN_CM: [f64; 3] = [-311.0, -70472.0, 809.4999387264252];
pub const NH04_YAW_DEG: f64 = 90.0;

pub const REPORT_HEADER: &str = "\
tile scan: all json except vegetation levels (spawners are suspected in <Map>_T<nn>_Dynamic; _Fixed, _Lighting and the other levels are scanned too because that split is unconfirmed, UNVERIFIED 115)";

/// Env var that overrides the archetype table path.
pub const ARCHETYPES_JSON_ENV: &str = "DI2_ARCHETYPES_JSON";

/// Default archetype table, relative to the repo root. Gitignored and generated locally by
/// `tools/convert_archetypes.py`; the same file `tests/archetypes.rs` reads.
pub const ARCHETYPES_JSON_REL: &str = "game/data/archetypes.json";

/// Where the allowed archetype ids came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchetypeIdSource {
    /// Every row `name` of this archetype table.
    Table(PathBuf),
    /// [`FALLBACK_ARCHETYPE_IDS`], with the reason the table was not used.
    Fallback(String),
}

/// The allowed archetype id set and its source.
#[derive(Debug, Clone)]
pub struct ArchetypeIds {
    pub ids: HashSet<String>,
    pub source: ArchetypeIdSource,
}

#[derive(serde::Deserialize)]
struct ArchetypeName {
    name: String,
}

impl ArchetypeIds {
    pub fn fallback(reason: impl Into<String>) -> Self {
        ArchetypeIds {
            ids: FALLBACK_ARCHETYPE_IDS.iter().map(|s| s.to_string()).collect(),
            source: ArchetypeIdSource::Fallback(reason.into()),
        }
    }

    /// Reads only the `name` of each row of an archetype table (the [`crate::archetype::Archetype`]
    /// list). A missing file gives the fallback set; a file that exists but does not parse, or has no
    /// rows, also gives the fallback set with the reason.
    pub fn from_path(path: &Path) -> Self {
        let src = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return Self::fallback("archetypes.json not found"),
        };
        match serde_json::from_str::<Vec<ArchetypeName>>(&src) {
            Ok(rows) if !rows.is_empty() => ArchetypeIds {
                ids: rows.into_iter().map(|r| r.name).collect(),
                source: ArchetypeIdSource::Table(path.to_path_buf()),
            },
            Ok(_) => Self::fallback(format!("archetypes.json has no rows: {}", path.display())),
            Err(e) => Self::fallback(format!("archetypes.json unreadable: {}: {e}", path.display())),
        }
    }

    /// `DI2_ARCHETYPES_JSON` when set, otherwise `<repo>/game/data/archetypes.json`.
    pub fn default_path() -> PathBuf {
        match std::env::var_os(ARCHETYPES_JSON_ENV) {
            Some(p) if !p.is_empty() => PathBuf::from(p),
            _ => Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(ARCHETYPES_JSON_REL),
        }
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    pub fn is_table(&self) -> bool {
        matches!(self.source, ArchetypeIdSource::Table(_))
    }

    /// Ids sorted by name, then rotated left by `seed % len`. Fixed for a given table and seed. The
    /// same rule is in game/scripts/horde_director.gd.
    pub fn seeded_order(&self, seed: u64) -> Vec<String> {
        let mut ids: Vec<String> = self.ids.iter().cloned().collect();
        ids.sort();
        if !ids.is_empty() {
            let k = (seed % ids.len() as u64) as usize;
            ids.rotate_left(k);
        }
        ids
    }

    /// One report line: `archetype ids: N from <path>` or `archetype ids: 22 fallback (<reason>)`.
    pub fn describe(&self) -> String {
        match &self.source {
            ArchetypeIdSource::Table(p) => format!("archetype ids: {} from {}", self.ids.len(), p.display()),
            ArchetypeIdSource::Fallback(why) => format!("archetype ids: {} fallback ({why})", self.ids.len()),
        }
    }
}

/// Process-wide allowed archetype ids, loaded once from [`ArchetypeIds::default_path`].
pub fn archetype_ids() -> &'static ArchetypeIds {
    static IDS: OnceLock<ArchetypeIds> = OnceLock::new();
    IDS.get_or_init(|| ArchetypeIds::from_path(&ArchetypeIds::default_path()))
}

pub fn archetype_allowed(id: &str) -> bool {
    archetype_ids().contains(id)
}

/// Seed for [`ArchetypeIds::seeded_order`] when the stand-in borrows ids from a loaded table.
pub const STAND_IN_ID_SEED: u64 = 1;

/// Stand-in id -> table id, used only for the invented stand-in when `ids` came from a loaded table
/// with rows. Fallback id at index `i` maps to `seeded_order(STAND_IN_ID_SEED)[i % len]`. Ids that are
/// not in [`FALLBACK_ARCHETYPE_IDS`] are not mapped. Empty when `ids` is the fallback set.
pub fn stand_in_id_map(ids: &ArchetypeIds) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if !ids.is_table() || ids.ids.is_empty() {
        return map;
    }
    let order = ids.seeded_order(STAND_IN_ID_SEED);
    for (i, fb) in FALLBACK_ARCHETYPE_IDS.iter().enumerate() {
        map.insert(fb.to_string(), order[i % order.len()].clone());
    }
    map
}

/// `NH_04_T01_Dynamic.json` -> `Some("Dynamic")`. Other level files are still scanned.
pub fn tile_role(file_name: &str) -> Option<&'static str> {
    let stem = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let stem = stem.strip_suffix(".json").unwrap_or(stem);
    let idx = stem.rfind("_T")?;
    let rest = &stem[idx + 2..];
    let (num, kind) = rest.split_once('_')?;
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    match kind {
        "Fixed" => Some("Fixed"),
        "Dynamic" => Some("Dynamic"),
        "Lighting" => Some("Lighting"),
        _ => None,
    }
}

/// UE transform. Without `AttachParent`, `location_cm` is the root component's `RelativeLocation`
/// (already world space). With `AttachParent`, it is that offset composed through the parent chain.
/// `rotation_deg` is the root component's own `RelativeRotation` in degrees (pitch, yaw, roll).
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnTransform {
    pub location_cm: [f64; 3],
    pub rotation_deg: [f64; 3],
}

impl SpawnTransform {
    /// Godot metres. `yaw_deg` is applied after the glTF y/z swap, about the origin.
    pub fn to_godot(&self, origin_cm: [f64; 3], yaw_deg: f64) -> Vec3 {
        let dx = (self.location_cm[0] - origin_cm[0]) * 0.01;
        let dy = (self.location_cm[1] - origin_cm[1]) * 0.01;
        let dz = (self.location_cm[2] - origin_cm[2]) * 0.01;
        let (sx, sy, sz) = (dx, dz, dy);
        let a = yaw_deg.to_radians();
        let (c, s) = (a.cos(), a.sin());
        Vec3::new((c * sx + s * sz) as f32, sy as f32, (-s * sx + c * sz) as f32)
    }
}

/// One spawner, whatever its class. Unknown classes stay in the list with `unknown_type`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnRecord {
    pub id: String,
    pub transform: SpawnTransform,
    pub archetype_id: String,
    /// Text stored on the export before the allowlist check. Empty when the field was absent.
    pub archetype_raw: String,
    pub spawn_table: String,
    pub table_resolved: bool,
    pub spawner_type: String,
    pub unknown_type: bool,
    /// Where the archetype came from or where the chain stopped ([`spawn_read::ArchetypeChain::describe`]).
    /// Empty when no `Content` root was known (single files, fixtures).
    pub archetype_chain: String,
    /// The chain end alone, for grouping ([`spawn_read::ArchetypeChain::end_text`]).
    pub archetype_chain_end: String,
    /// Cooked fields that were absent, filled with a stand-in because the class default is not in the JSON.
    pub fallback_fields: Vec<String>,
    pub source: String,
    /// An emergent activity actor (owns a `ParticipantSlot`); a population point like a spawn point.
    pub activity: bool,
}

/// Stand-in pacing. See recon/UNVERIFIED.md. Overridable from a `HordeDirectorBudget` export.
#[derive(Debug, Clone, PartialEq)]
pub struct Budget {
    pub max_active: u32,
    pub min_spawn_distance: f32,
    pub max_spawn_distance: f32,
    pub despawn_distance: f32,
    pub spawn_interval: f32,
    pub max_spawns_per_tick: u32,
    pub point_cooldown: f32,
    pub view_half_angle_deg: f32,
    pub seed: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_active: 8,
            min_spawn_distance: 8.0,
            max_spawn_distance: 42.0,
            despawn_distance: 64.0,
            spawn_interval: 2.5,
            max_spawns_per_tick: 1,
            point_cooldown: 12.0,
            view_half_angle_deg: 55.0,
            seed: 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpawnLayout {
    /// True when the export list carries the stand-in marker (the committed invented fallback).
    pub stand_in: bool,
    pub records: Vec<SpawnRecord>,
    /// Spawn-table id -> archetype weights. Ids outside [`archetype_ids`] are dropped.
    pub tables: HashMap<String, Vec<(String, f32)>>,
    pub budget: Budget,
    pub budget_specified: bool,
    /// The allowed id set this layout was parsed against; the director and report use it.
    pub ids: ArchetypeIds,
}

impl SpawnLayout {
    /// Stand-in positions are an invented local centimetre frame. A real dump uses the NH_04 map frame.
    pub fn frame(&self) -> ([f64; 3], f64) {
        if self.stand_in {
            ([0.0; 3], 0.0)
        } else {
            (NH04_ORIGIN_CM, NH04_YAW_DEG)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum HordeOrder {
    Spawn {
        id: u64,
        record_id: String,
        archetype_id: String,
        position: Vec3,
        yaw: f32,
    },
    Despawn {
        id: u64,
    },
}

struct Live {
    id: u64,
    record_id: String,
    position: Vec3,
}

struct Point {
    record_id: String,
    position: Vec3,
    yaw: f32,
    archetype_id: String,
    table: String,
    cooldown: f32,
    occupant: Option<u64>,
}

pub struct HordeDirector {
    ids: ArchetypeIds,
    points: Vec<Point>,
    tables: HashMap<String, Vec<(String, f32)>>,
    budget: Budget,
    live: Vec<Live>,
    next_id: u64,
    spawn_wait: f32,
    rng: Rng,
    cos_half: f32,
}

impl HordeDirector {
    pub fn new(layout: &SpawnLayout) -> Self {
        Self::with_seed(layout, layout.budget.seed)
    }

    pub fn with_seed(layout: &SpawnLayout, seed: u64) -> Self {
        let (origin, yaw) = layout.frame();
        let ids = &layout.ids;
        let points = layout
            .records
            .iter()
            .filter(|r| !r.unknown_type && (POPULATION_TYPES.contains(&r.spawner_type.as_str()) || r.activity))
            .map(|r| Point {
                record_id: r.id.clone(),
                position: r.transform.to_godot(origin, yaw),
                yaw: r.transform.rotation_deg[1] as f32,
                archetype_id: if ids.contains(&r.archetype_id) {
                    r.archetype_id.clone()
                } else {
                    String::new()
                },
                table: r.spawn_table.clone(),
                cooldown: 0.0,
                occupant: None,
            })
            .collect();
        let half = layout.budget.view_half_angle_deg.to_radians();
        Self {
            ids: layout.ids.clone(),
            points,
            tables: layout.tables.clone(),
            budget: layout.budget.clone(),
            live: Vec::new(),
            next_id: 1,
            spawn_wait: 0.0,
            rng: Rng::new(seed),
            cos_half: half.cos(),
        }
    }

    pub fn budget_mut(&mut self) -> &mut Budget {
        &mut self.budget
    }

    pub fn active(&self) -> usize {
        self.live.len()
    }

    pub fn notify_gone(&mut self, id: u64) {
        if let Some(i) = self.live.iter().position(|z| z.id == id) {
            let record_id = self.live[i].record_id.clone();
            self.live.remove(i);
            self.release(&record_id);
        }
    }

    /// Advance the population. `forward` is the horizontal look direction.
    pub fn tick(&mut self, dt: f32, player: Vec3, forward: Vec3) -> Vec<HordeOrder> {
        for p in &mut self.points {
            p.cooldown = (p.cooldown - dt).max(0.0);
        }
        let mut orders = Vec::new();
        let despawn_at = self.budget.despawn_distance;
        let gone: Vec<u64> = self
            .live
            .iter()
            .filter(|z| flat_dist(z.position, player) > despawn_at)
            .map(|z| z.id)
            .collect();
        for id in gone {
            if let Some(i) = self.live.iter().position(|z| z.id == id) {
                let record_id = self.live[i].record_id.clone();
                self.live.remove(i);
                self.release(&record_id);
                orders.push(HordeOrder::Despawn { id });
            }
        }

        self.spawn_wait -= dt;
        if self.spawn_wait > 0.0 || self.budget.max_spawns_per_tick == 0 {
            return orders;
        }
        let room = self.budget.max_active.saturating_sub(self.live.len() as u32);
        let n = room.min(self.budget.max_spawns_per_tick);
        let mut spawned = 0u32;
        for _ in 0..n {
            if let Some(order) = self.try_spawn(player, forward) {
                orders.push(order);
                spawned += 1;
            } else {
                break;
            }
        }
        if spawned > 0 {
            self.spawn_wait = self.budget.spawn_interval;
        }
        orders
    }

    fn release(&mut self, record_id: &str) {
        if let Some(p) = self.points.iter_mut().find(|p| p.record_id == record_id) {
            p.occupant = None;
            p.cooldown = self.budget.point_cooldown;
        }
    }

    fn try_spawn(&mut self, player: Vec3, forward: Vec3) -> Option<HordeOrder> {
        let min_d = self.budget.min_spawn_distance;
        let max_d = self.budget.max_spawn_distance;
        let mut candidates: Vec<usize> = Vec::new();
        for (i, p) in self.points.iter().enumerate() {
            if p.occupant.is_some() || p.cooldown > 0.0 {
                continue;
            }
            let d = flat_dist(p.position, player);
            if d < min_d || d > max_d {
                continue;
            }
            if !self.can_spawn(i) {
                continue;
            }
            candidates.push(i);
        }
        if candidates.is_empty() {
            return None;
        }
        let hidden: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&i| !in_view(player, forward, self.points[i].position, self.cos_half))
            .collect();
        let pool = if hidden.is_empty() { candidates } else { hidden };
        let weights: Vec<f32> = pool
            .iter()
            .map(|&i| 1.0 / flat_dist(self.points[i].position, player).max(1.0))
            .collect();
        let i = pool[pick_weighted(&mut self.rng, &weights)];
        let archetype = self.archetype_for(i)?;
        let id = self.next_id;
        self.next_id += 1;
        let record_id = self.points[i].record_id.clone();
        let position = self.points[i].position;
        let yaw = self.points[i].yaw;
        self.points[i].occupant = Some(id);
        self.live.push(Live { id, record_id: record_id.clone(), position });
        Some(HordeOrder::Spawn { id, record_id, archetype_id: archetype, position, yaw })
    }

    fn can_spawn(&self, index: usize) -> bool {
        if self.ids.contains(&self.points[index].archetype_id) {
            return true;
        }
        self.tables.get(&self.points[index].table).map(|w| !w.is_empty()).unwrap_or(false)
    }

    fn archetype_for(&mut self, index: usize) -> Option<String> {
        let own = self.points[index].archetype_id.clone();
        if self.ids.contains(&own) {
            return Some(own);
        }
        let table = self.points[index].table.clone();
        let weights = self.tables.get(&table)?;
        if weights.is_empty() {
            return None;
        }
        let w: Vec<f32> = weights.iter().map(|(_, n)| *n).collect();
        let pick = pick_weighted(&mut self.rng, &w);
        Some(weights[pick].0.clone())
    }
}

fn flat_dist(a: Vec3, b: Vec3) -> f32 {
    let d = a - b;
    Vec3::new(d.x, 0.0, d.z).length()
}

fn in_view(player: Vec3, forward: Vec3, point: Vec3, cos_half: f32) -> bool {
    let mut to = point - player;
    to.y = 0.0;
    let mut fwd = forward;
    fwd.y = 0.0;
    if to.length_squared() < 1e-8 || fwd.length_squared() < 1e-8 {
        return false;
    }
    to.normalize().dot(fwd.normalize()) >= cos_half - 1e-4
}

fn pick_weighted(rng: &mut Rng, weights: &[f32]) -> usize {
    let total: f32 = weights.iter().copied().sum();
    if total <= 0.0 || weights.is_empty() {
        return 0;
    }
    let mut t = rng.f32() * total;
    for (i, w) in weights.iter().enumerate() {
        t -= *w;
        if t <= 0.0 {
            return i;
        }
    }
    weights.len() - 1
}

/// Parse a CUE4Parse export array against the process-wide [`archetype_ids`].
pub fn parse_layout(exports: &[Value]) -> Result<SpawnLayout, String> {
    parse_layout_with(exports, archetype_ids())
}

fn is_stand_in(e: &Value) -> bool {
    let name = e.get("Name").and_then(|v| v.as_str()).unwrap_or("");
    let props = e.get("Properties").cloned().unwrap_or(Value::Null);
    prop_bool(&props, "StandIn") || name == "INVENTED_STAND_IN"
}

/// Parse a CUE4Parse export array. A real dump and the invented stand-in share this path. Only the
/// stand-in has its ids mapped onto a loaded table ([`stand_in_id_map`]); a real dump never is.
pub fn parse_layout_with(exports: &[Value], ids: &ArchetypeIds) -> Result<SpawnLayout, String> {
    parse_layout_rooted(exports, ids, &mut Packages::new(None))
}

/// [`parse_layout_with`], following archetype chains into other packages of `packages`' root.
pub fn parse_layout_rooted(exports: &[Value], ids: &ArchetypeIds, packages: &mut Packages) -> Result<SpawnLayout, String> {
    let id_map = if exports.iter().any(is_stand_in) { stand_in_id_map(ids) } else { HashMap::new() };
    let mut records = Vec::new();
    let mut tables: HashMap<String, Vec<(String, f32)>> = HashMap::new();
    let mut budget = Budget::default();
    let mut budget_specified = false;
    let mut stand_in = false;
    for (i, e) in exports.iter().enumerate() {
        let ty = e.get("Type").and_then(|v| v.as_str()).unwrap_or("");
        let name = e.get("Name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let props = e.get("Properties").cloned().unwrap_or(Value::Null);
        if is_stand_in(e) {
            stand_in = true;
        }
        if ty == "HordeDirectorBudget" {
            apply_budget(&mut budget, &props);
            budget_specified = true;
            continue;
        }
        if !spawner_candidate(ty, &props) {
            continue;
        }
        let placed = world_transform(exports, i);
        let (location, rotation, mut fallback_fields) = match placed {
            Some(p) => (p.location, p.local_rot, p.fallbacks),
            None => ([0.0; 3], [0.0; 3], vec!["RootComponent".to_string()]),
        };
        let (archetype_id, archetype_raw, archetype_missing) = archetype_of(&props, ids, &id_map);
        let (archetype_chain, archetype_chain_end) = if packages.root().is_some() {
            let chain = spawn_read::archetype_chain(exports, i, packages);
            (chain.describe(), chain.end_text())
        } else {
            (String::new(), String::new())
        };
        let activity = is_activity(&props);
        if archetype_missing {
            fallback_fields.push("Archetype".to_string());
        }
        let table = table_link(exports, &props);
        let mut spawn_table = table.name;
        let mut table_resolved = table.resolved;
        if let Some(w) = weights_of(&props) {
            let kept: Vec<(String, f32)> = w
                .into_iter()
                .map(|(id, n)| (id_map.get(&id).cloned().unwrap_or(id), n))
                .filter(|(id, _)| ids.contains(id))
                .collect();
            if !kept.is_empty() {
                tables.insert(name.clone(), kept);
            }
        }
        if spawn_table.is_empty() && tables.contains_key(&name) {
            spawn_table = name.clone();
            table_resolved = true;
        }
        records.push(SpawnRecord {
            id: name,
            transform: SpawnTransform { location_cm: location, rotation_deg: rotation },
            archetype_id,
            archetype_raw,
            spawn_table,
            table_resolved,
            spawner_type: ty.to_string(),
            unknown_type: !KNOWN_SPAWNER_TYPES.contains(&ty) && !activity,
            archetype_chain,
            archetype_chain_end,
            fallback_fields,
            source: String::new(),
            activity,
        });
    }
    // A cluster that lists its points by reference fills an empty spawn-table link.
    for e in exports {
        let ty = e.get("Type").and_then(|v| v.as_str()).unwrap_or("");
        if ty != "BP_Horde_TSL_ZombieCluster_C" {
            continue;
        }
        let cluster = e.get("Name").and_then(|v| v.as_str()).unwrap_or("");
        let Some(list) = e.get("Properties").and_then(|p| p.get("SpawnPoints")).and_then(|v| v.as_array()) else {
            continue;
        };
        for r in list {
            let Some(idx) = resolve_ref(exports, r) else { continue };
            let point_name = exports[idx].get("Name").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(rec) = records.iter_mut().find(|rec| rec.id == point_name) {
                if rec.spawn_table.is_empty() {
                    rec.spawn_table = cluster.to_string();
                    rec.table_resolved = true;
                }
            }
        }
    }
    for rec in &mut records {
        if rec.spawn_table.is_empty() && !rec.table_resolved && !rec.fallback_fields.iter().any(|f| f == "SpawnTable")
        {
            let props_missing = exports.iter().find(|e| e.get("Name").and_then(|v| v.as_str()) == Some(rec.id.as_str()));
            let present = props_missing
                .and_then(|e| e.get("Properties"))
                .map(|p| TABLE_KEYS.iter().any(|k| p.get(*k).is_some()))
                .unwrap_or(false);
            if !present {
                rec.fallback_fields.push("SpawnTable".to_string());
            }
        }
    }
    Ok(SpawnLayout { stand_in, records, tables, budget, budget_specified, ids: ids.clone() })
}

pub fn parse_layout_str(text: &str) -> Result<SpawnLayout, String> {
    parse_layout_str_with(text, archetype_ids())
}

pub fn parse_layout_str_with(text: &str, ids: &ArchetypeIds) -> Result<SpawnLayout, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let exports = value.as_array().ok_or_else(|| "CUE4Parse dump is not a JSON array".to_string())?;
    parse_layout_with(exports, ids)
}

pub struct Scan {
    pub layout: SpawnLayout,
    pub files_scanned: Vec<String>,
    pub files_skipped: Vec<(String, String)>,
    /// CUE4Parse `Content` root above the scanned directory, when there is one.
    pub content_root: Option<PathBuf>,
}

/// [`scan_dir_with`] against the process-wide [`archetype_ids`].
pub fn scan_dir(dir: &Path) -> Result<Scan, String> {
    scan_dir_with(dir, archetype_ids())
}

/// Read every `.json` file under `dir` (tiles and any other export). Reads only: creates nothing.
pub fn scan_dir_with(dir: &Path, ids: &ArchetypeIds) -> Result<Scan, String> {
    if !dir.is_dir() {
        return Err(format!("not a directory: {}", dir.display()));
    }
    let mut paths = Vec::new();
    collect_json(dir, &mut paths)?;
    paths.sort();
    let mut layout = SpawnLayout {
        stand_in: false,
        records: Vec::new(),
        tables: HashMap::new(),
        budget: Budget::default(),
        budget_specified: false,
        ids: ids.clone(),
    };
    let mut files_scanned = Vec::new();
    let mut files_skipped = Vec::new();
    let mut packages = Packages::new(spawn_read::content_root_of(dir));
    for path in paths {
        let rel = path.strip_prefix(dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        if spawn_read::is_vegetation_level(&rel) {
            files_skipped.push((rel, "vegetation level (not read for spawners)".to_string()));
            continue;
        }
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                files_skipped.push((rel, e.to_string()));
                continue;
            }
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                files_skipped.push((rel, e.to_string()));
                continue;
            }
        };
        let Some(exports) = value.as_array() else {
            files_skipped.push((rel, "not a CUE4Parse export array".to_string()));
            continue;
        };
        let part = parse_layout_rooted(exports, ids, &mut packages)?;
        files_scanned.push(rel.clone());
        if part.stand_in {
            layout.stand_in = true;
        }
        if part.budget_specified {
            layout.budget = part.budget;
            layout.budget_specified = true;
        }
        for (k, v) in part.tables {
            layout.tables.entry(k).or_insert(v);
        }
        for mut rec in part.records {
            rec.source = rel.clone();
            layout.records.push(rec);
        }
    }
    let content_root = packages.root().map(Path::to_path_buf);
    Ok(Scan { layout, files_scanned, files_skipped, content_root })
}

/// Text report for [`scan_dir`], using the id set the layout was parsed against. The caller prints it;
/// this function does not touch the filesystem.
pub fn format_report(layout: &SpawnLayout, files_scanned: &[String], files_skipped: &[(String, String)]) -> String {
    let mut out = String::new();
    out.push_str(REPORT_HEADER);
    out.push('\n');
    out.push_str(&layout.ids.describe());
    out.push('\n');
    out.push_str(&format!("files scanned: {}\n", files_scanned.len()));
    for f in files_scanned {
        out.push_str(&format!("  {f}\n"));
    }
    out.push_str(&format!("files skipped: {}\n", files_skipped.len()));
    for (f, why) in files_skipped {
        out.push_str(&format!("  {f} | {why}\n"));
    }
    out.push_str(&format!("spawners found: {}\n", layout.records.len()));
    for r in &layout.records {
        out.push_str(&format!("  {} | {} | {}\n", r.id, r.spawner_type, r.source));
    }
    let unknown: Vec<&SpawnRecord> = layout.records.iter().filter(|r| r.unknown_type).collect();
    out.push_str(&format!("spawners of unknown type: {}\n", unknown.len()));
    for r in unknown {
        out.push_str(&format!("  {} | {}\n", r.id, r.spawner_type));
    }
    let no_arch: Vec<&SpawnRecord> = layout.records.iter().filter(|r| !layout.ids.contains(&r.archetype_id)).collect();
    out.push_str(&format!("spawners with no matching archetype id: {}\n", no_arch.len()));
    for r in no_arch {
        out.push_str(&format!("  {} | {}\n", r.id, r.archetype_raw));
    }
    let mut filled = 0usize;
    let mut filled_lines = String::new();
    for r in &layout.records {
        for field in &r.fallback_fields {
            filled += 1;
            filled_lines.push_str(&format!("  {} | {field}\n", r.id));
        }
    }
    out.push_str(&format!("fields filled from fallbacks: {filled}\n"));
    out.push_str(&filled_lines);
    let resolved: Vec<&SpawnRecord> = layout.records.iter().filter(|r| r.table_resolved && !r.spawn_table.is_empty()).collect();
    out.push_str(&format!("spawn tables resolved: {}\n", resolved.len()));
    for r in resolved {
        out.push_str(&format!("  {} | {}\n", r.id, r.spawn_table));
    }
    out
}

/// Ticket 2 sections, printed after [`format_report`] when the scan sat under a CUE4Parse `Content`
/// root: spawners per file (evidence for UNVERIFIED 115), where each archetype chain stopped, and the
/// attribute values of every allowed archetype id. Empty without a root, so fixture reports are unchanged.
pub fn format_read_report(scan: &Scan) -> String {
    let Some(root) = scan.content_root.clone() else { return String::new() };
    let layout = &scan.layout;
    let mut out = String::new();
    out.push_str(&format!("content root: {}\n", root.display()));

    let mut per_file: Vec<(String, usize, usize)> = Vec::new();
    for r in &layout.records {
        if r.unknown_type {
            continue;
        }
        let points = usize::from(r.spawner_type == "BP_ZombieSpawnPoint_C");
        match per_file.iter_mut().find(|(f, _, _)| *f == r.source) {
            Some(e) => {
                e.1 += 1;
                e.2 += points;
            }
            None => per_file.push((r.source.clone(), 1, points)),
        }
    }
    per_file.sort();
    out.push_str(&format!("files with known spawners: {}\n", per_file.len()));
    for (f, n, points) in &per_file {
        let role = tile_role(f).unwrap_or("not a tile");
        out.push_str(&format!("  {f} | known spawners {n} | BP_ZombieSpawnPoint_C {points} | {role}\n"));
    }
    let dynamic = per_file.iter().filter(|(f, _, _)| tile_role(f) == Some("Dynamic")).count();
    out.push_str(&format!(
        "UNVERIFIED 115: files with known spawners that are _Dynamic tiles: {dynamic} of {} (this export only)\n",
        per_file.len()
    ));

    let known: Vec<&SpawnRecord> = layout.records.iter().filter(|r| !r.unknown_type).collect();
    let mut ends: Vec<(String, usize)> = Vec::new();
    for r in &known {
        match ends.iter_mut().find(|(e, _)| *e == r.archetype_chain_end) {
            Some(e) => e.1 += 1,
            None => ends.push((r.archetype_chain_end.clone(), 1)),
        }
    }
    ends.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.push_str(&format!("archetype chain ends (known spawners {}):\n", known.len()));
    for (e, n) in &ends {
        out.push_str(&format!("  {n} | {e}\n"));
    }
    out.push_str("archetype chains:\n");
    for r in &known {
        out.push_str(&format!("  {} | {}\n", r.id, r.archetype_chain));
    }

    let mut packages = Packages::new(Some(root));
    let mut ids: Vec<&String> = layout.ids.ids.iter().collect();
    ids.sort();
    let (mut class_n, mut variant_n, mut type_n, mut type_missing_n, mut native_n) = (0, 0, 0, 0, 0);
    let mut body = String::new();
    for id in ids {
        let read = spawn_read::resolve_attributes(&mut packages, id);
        let name = |p: &Option<PathBuf>| {
            p.as_ref()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "none in export".to_string())
        };
        body.push_str(&format!("  {id} | class {} | variant {}\n", name(&read.class_asset), name(&read.variant_asset)));
        for v in &read.values {
            match v.source {
                spawn_read::AttrSource::Class => class_n += 1,
                spawn_read::AttrSource::Variant => variant_n += 1,
                spawn_read::AttrSource::TypeDefault(_) => type_n += 1,
                spawn_read::AttrSource::TypeAssetNotDumped(_) => type_missing_n += 1,
                spawn_read::AttrSource::Native(_) => native_n += 1,
            }
            body.push_str(&format!("    {}\n", v.describe()));
        }
    }
    out.push_str(&format!(
        "attribute values: class asset {class_n} | variant asset {variant_n} | type DefaultValue {type_n} | type asset not in export {type_missing_n} | {} {native_n}\n",
        spawn_read::NATIVE_DEFAULT
    ));
    out.push_str(&body);
    out
}

/// Extracted tile directory when it contains spawners, otherwise a single spawning dump, otherwise
/// the invented stand-in. Real positions are whatever those JSON files contain.
pub fn load_nh04(repo_root: &Path) -> Result<SpawnLayout, String> {
    load_nh04_with(repo_root, archetype_ids())
}

/// [`load_nh04`] against an explicit id set.
pub fn load_nh04_with(repo_root: &Path, ids: &ArchetypeIds) -> Result<SpawnLayout, String> {
    let dir = repo_root.join(NH04_EXTRACT_DIR);
    if dir.is_dir() {
        let scan = scan_dir_with(&dir, ids)?;
        if !scan.layout.records.is_empty() {
            return Ok(scan.layout);
        }
    }
    let dump = repo_root.join(NH04_SPAWNING_DUMP);
    if dump.is_file() {
        let text = fs::read_to_string(&dump).map_err(|e| format!("{}: {e}", dump.display()))?;
        return parse_layout_str_with(&text, ids);
    }
    let path = repo_root.join(FALLBACK_REL);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_layout_str_with(&text, ids)
}

fn collect_json(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let rd = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for ent in rd {
        let ent = ent.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = ent.path();
        if path.is_dir() {
            collect_json(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            out.push(path);
        }
    }
    Ok(())
}

struct Placed {
    location: [f64; 3],
    local_rot: [f64; 3],
    world_rows: [[f64; 3]; 3],
    fallbacks: Vec<String>,
}

fn spawner_candidate(ty: &str, props: &Value) -> bool {
    if ty.is_empty() || ty.contains("Component") || ty == "HordeDirectorBudget" {
        return false;
    }
    props.get("RootComponent").is_some() || ty.ends_with("_C")
}

fn apply_budget(budget: &mut Budget, props: &Value) {
    if let Some(n) = prop_u32(props, "MaxActive") {
        budget.max_active = n;
    }
    if let Some(n) = prop_f32(props, "MinSpawnDistance") {
        budget.min_spawn_distance = n;
    }
    if let Some(n) = prop_f32(props, "MaxSpawnDistance") {
        budget.max_spawn_distance = n;
    }
    if let Some(n) = prop_f32(props, "DespawnDistance") {
        budget.despawn_distance = n;
    }
    if let Some(n) = prop_f32(props, "SpawnInterval") {
        budget.spawn_interval = n;
    }
    if let Some(n) = prop_u32(props, "MaxSpawnsPerTick") {
        budget.max_spawns_per_tick = n;
    }
    if let Some(n) = prop_f32(props, "PointCooldown") {
        budget.point_cooldown = n;
    }
    if let Some(n) = prop_f32(props, "ViewHalfAngleDeg") {
        budget.view_half_angle_deg = n;
    }
    if let Some(n) = prop_u32(props, "Seed") {
        budget.seed = n as u64;
    }
}

fn archetype_of(props: &Value, ids: &ArchetypeIds, id_map: &HashMap<String, String>) -> (String, String, bool) {
    for key in ARCHETYPE_KEYS {
        if let Some(raw) = prop_text(props, key) {
            let mapped = id_map.get(&raw).cloned().unwrap_or_else(|| raw.clone());
            let id = if ids.contains(&mapped) { mapped } else { String::new() };
            return (id, raw, false);
        }
    }
    (String::new(), String::new(), true)
}

struct TableHit {
    name: String,
    resolved: bool,
}

fn table_link(exports: &[Value], props: &Value) -> TableHit {
    for key in TABLE_KEYS {
        let Some(v) = props.get(*key) else { continue };
        if let Some(s) = v.as_str() {
            if !s.is_empty() {
                return TableHit { name: s.to_string(), resolved: true };
            }
            return TableHit { name: String::new(), resolved: false };
        }
        if let Some(i) = resolve_ref(exports, v) {
            if let Some(name) = exports[i].get("Name").and_then(|n| n.as_str()) {
                if !name.is_empty() {
                    return TableHit { name: name.to_string(), resolved: true };
                }
            }
        }
        return TableHit { name: String::new(), resolved: false };
    }
    TableHit { name: String::new(), resolved: false }
}

fn weights_of(props: &Value) -> Option<Vec<(String, f32)>> {
    let v = props.get("ArchetypeWeights")?;
    let mut out = Vec::new();
    if let Some(arr) = v.as_array() {
        for item in arr {
            let key = item.get("Key").and_then(|k| k.as_str()).unwrap_or("");
            let val = item.get("Value").and_then(|n| n.as_f64()).unwrap_or(0.0) as f32;
            if !key.is_empty() && val > 0.0 {
                out.push((key.to_string(), val));
            }
        }
    } else if let Some(obj) = v.as_object() {
        for (k, n) in obj {
            if let Some(val) = n.as_f64() {
                if val > 0.0 {
                    out.push((k.clone(), val as f32));
                }
            }
        }
    }
    Some(out)
}

fn world_transform(exports: &[Value], actor: usize) -> Option<Placed> {
    let props = exports.get(actor)?.get("Properties")?;
    let root = resolve_ref(exports, props.get("RootComponent")?)?;
    component_world(exports, root, 0)
}

fn component_world(exports: &[Value], index: usize, depth: usize) -> Option<Placed> {
    if depth > 16 {
        return None;
    }
    let export = exports.get(index)?;
    let cname = export.get("Name").and_then(|v| v.as_str()).unwrap_or("component");
    let empty = Value::Null;
    let props = export.get("Properties").unwrap_or(&empty);
    let (local, loc_falls) = read_vec3_field(props, "RelativeLocation", ["X", "Y", "Z"]);
    let (rot, rot_falls) = read_rot_field(props, "RelativeRotation");
    let mut fallbacks = Vec::new();
    for f in loc_falls.into_iter().chain(rot_falls) {
        fallbacks.push(format!("{cname}.{f}"));
    }
    let local_rows = rotator_rows(rot);
    if props.get("AttachParent").is_some() {
        match props.get("AttachParent").and_then(|r| resolve_ref(exports, r)) {
            Some(parent) if parent != index => {
                let parent_placed = component_world(exports, parent, depth + 1)?;
                let rotated = mul_rows(parent_placed.world_rows, local);
                fallbacks.extend(parent_placed.fallbacks);
                return Some(Placed {
                    location: [
                        rotated[0] + parent_placed.location[0],
                        rotated[1] + parent_placed.location[1],
                        rotated[2] + parent_placed.location[2],
                    ],
                    local_rot: rot,
                    world_rows: mat_mul(local_rows, parent_placed.world_rows),
                    fallbacks,
                });
            }
            _ => fallbacks.push(format!("{cname}.AttachParent")),
        }
    }
    Some(Placed { location: local, local_rot: rot, world_rows: local_rows, fallbacks })
}

fn rotator_rows(deg: [f64; 3]) -> [[f64; 3]; 3] {
    let (p, y, ro) = (deg[0].to_radians(), deg[1].to_radians(), deg[2].to_radians());
    let (sp, cp) = (p.sin(), p.cos());
    let (sy, cy) = (y.sin(), y.cos());
    let (sr, cr) = (ro.sin(), ro.cos());
    [
        [cp * cy, cp * sy, sp],
        [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp],
        [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp],
    ]
}

fn mat_mul(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    o
}

fn mul_rows(m: [[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2],
        m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2],
        m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2],
    ]
}

fn resolve_ref(exports: &[Value], reference: &Value) -> Option<usize> {
    let path = reference.get("ObjectPath").and_then(|v| v.as_str()).unwrap_or("");
    if let Some((_, tail)) = path.rsplit_once('.') {
        if let Ok(i) = tail.parse::<usize>() {
            if i < exports.len() {
                return Some(i);
            }
        }
    }
    let name = reference
        .get("ObjectName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .rsplit('\'')
        .nth(1)
        .unwrap_or("");
    if name.is_empty() {
        return None;
    }
    exports.iter().position(|e| e.get("Name").and_then(|n| n.as_str()) == Some(name))
}

fn read_vec3_field(props: &Value, key: &str, axes: [&str; 3]) -> ([f64; 3], Vec<String>) {
    let Some(v) = props.get(key) else {
        return ([0.0; 3], vec![key.to_string()]);
    };
    let mut out = [0.0; 3];
    let mut missing = Vec::new();
    for (i, axis) in axes.iter().enumerate() {
        if let Some(n) = v.get(*axis).and_then(|n| n.as_f64()) {
            out[i] = n;
        } else {
            missing.push(format!("{key}.{axis}"));
        }
    }
    (out, missing)
}

fn read_rot_field(props: &Value, key: &str) -> ([f64; 3], Vec<String>) {
    read_vec3_field(props, key, ["Pitch", "Yaw", "Roll"])
}

fn prop_text(props: &Value, key: &str) -> Option<String> {
    let v = props.get(key)?;
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if let Some(name) = v.get("ObjectName").and_then(|n| n.as_str()) {
        let inner = name.rsplit('\'').nth(1).unwrap_or(name);
        let inner = inner.strip_suffix("_C").unwrap_or(inner);
        let inner = inner.strip_prefix("BP_").unwrap_or(inner);
        return Some(inner.to_string());
    }
    if let Some(path) = v.get("ObjectPath").and_then(|n| n.as_str()) {
        let tail = path.rsplit('/').next().unwrap_or(path);
        let tail = tail.split('.').next().unwrap_or(tail);
        return Some(tail.to_string());
    }
    None
}

fn prop_bool(props: &Value, key: &str) -> bool {
    props.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn prop_f32(props: &Value, key: &str) -> Option<f32> {
    let v = props.get(key)?;
    v.as_f64().map(|n| n as f32)
}

fn prop_u32(props: &Value, key: &str) -> Option<u32> {
    let v = props.get(key)?;
    v.as_u64().or_else(|| v.as_f64().map(|n| n as u64)).map(|n| n as u32)
}
