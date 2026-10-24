//! Ticket 2: the real NH_04 spawn read from CUE4Parse JSON (the player's own export).
//!
//! Nothing here holds Dead Island 2 data. Every value is read at run time from the player's export
//! under a CUE4Parse `.../Content` root, and anything that is not in that export is reported as missing
//! rather than filled in.
//!
//! - Vegetation levels (`*_Vegetation.json`) are skipped ([`is_vegetation_level`]).
//! - `ObjectPath "/Game/<pkg>.N"` names export `N` (0-based) of package `<pkg>` ([`split_object_path`]);
//!   `<pkg>` maps to `<Content>/<pkg without /Game/>.json` ([`Packages`]). `/Script/...` is native code.
//! - Archetype: the spawner's component export decides it ([`archetype_chain`]). A spawn point goes
//!   `Cluster` -> the cluster's `AgentSource` component; an activity goes `ParticipantSlot` ->
//!   `Participant.SpawnableGroup`. A component that stores no archetype field falls back to its
//!   `Template` (the class default object, in the class's own package), and so on up the class default
//!   chain. The first link that is not in the export ends the chain and is named in the report.
//! - Attribute values ([`resolve_attributes`]): `DA_AIAttributeValues_<Class>` is the class asset and
//!   `DA_AIAttributeValues_<Class>_<Variant>` (when the player's export has it) overrides it field by
//!   field. A field whose `InitialSettings` does not set `bUseTypeDefaultValue: false` takes its
//!   `DA_AttributeType_*` asset's `DefaultValue`. A value the cooked data does not hold at all lives only
//!   in the native exe and is reported as [`NATIVE_DEFAULT`].

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Report text for a value that is only in the native exe (constructor default), not in cooked data.
pub const NATIVE_DEFAULT: &str = "native default, not yet read";

/// Field names that would carry an archetype directly on a component. Unverified (recon/UNVERIFIED.md 114).
pub const ARCHETYPE_KEYS: &[&str] = &["Archetype", "ArchetypeId", "ZombieArchetype", "SpawnArchetype", "ZombieType"];

/// `NH_04_Vegetation.json` and any other `*_Vegetation*` level: out of scope for the spawn read.
pub fn is_vegetation_level(rel: &str) -> bool {
    let stem = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
    stem.contains("_Vegetation")
}

/// Nearest ancestor of `dir` (or `dir` itself) named `Content`: the CUE4Parse export root.
pub fn content_root_of(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find(|p| p.file_name().map(|n| n == "Content").unwrap_or(false)).map(Path::to_path_buf)
}

/// `"/Game/A/B.12"` -> `("/Game/A/B", Some(12))`; `"/Script/Engine"` -> `("/Script/Engine", None)`.
/// The number is a 0-based export index of that package.
pub fn split_object_path(path: &str) -> (&str, Option<usize>) {
    if let Some((pkg, tail)) = path.rsplit_once('.') {
        if let Ok(i) = tail.parse::<usize>() {
            return (pkg, Some(i));
        }
    }
    (path, None)
}

/// Package files under one CUE4Parse `Content` root, each read at most once.
pub struct Packages {
    root: Option<PathBuf>,
    cache: HashMap<String, Option<Vec<Value>>>,
}

impl Packages {
    pub fn new(root: Option<PathBuf>) -> Self {
        Packages { root, cache: HashMap::new() }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// `/Game/A/B` -> `<root>/A/B.json`. `None` for native (`/Script/`) or engine packages.
    pub fn file_of(&self, pkg: &str) -> Option<PathBuf> {
        let rel = pkg.strip_prefix("/Game/")?;
        Some(self.root.as_ref()?.join(format!("{rel}.json")))
    }

    /// The package's export array, or `None` when it is not in the player's export.
    pub fn get(&mut self, pkg: &str) -> Option<&Vec<Value>> {
        if !self.cache.contains_key(pkg) {
            let loaded = self
                .file_of(pkg)
                .and_then(|f| fs::read_to_string(f).ok())
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v.as_array().cloned());
            self.cache.insert(pkg.to_string(), loaded);
        }
        self.cache.get(pkg).and_then(|o| o.as_ref())
    }
}

/// Where an archetype chain stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum ChainEnd {
    /// An archetype field was found; the raw text it holds.
    Archetype(String),
    /// The next link is a package that is not in the player's export.
    NotDumped { what: &'static str, package: String, export: Option<usize> },
    /// The next link is native code (`/Script/...`): its value is a [`NATIVE_DEFAULT`].
    Native(String),
    /// The asset is in the export, but no archetype field this reader knows is in it.
    NotRead { what: &'static str, package: String },
    /// The spawner has no link to follow.
    NoLink(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArchetypeChain {
    pub steps: Vec<String>,
    pub end: ChainEnd,
}

impl ArchetypeChain {
    /// The chain end without the steps, for grouping in the report.
    pub fn end_text(&self) -> String {
        match &self.end {
            ChainEnd::Archetype(raw) => format!("archetype field: {raw}"),
            ChainEnd::NotDumped { what, package, export } => match export {
                Some(i) => format!("{what} not in export: {package} (export {i})"),
                None => format!("{what} not in export: {package}"),
            },
            ChainEnd::Native(p) => format!("{NATIVE_DEFAULT}: {p}"),
            ChainEnd::NotRead { what, package } => format!("{what} in export, archetype field not read: {package}"),
            ChainEnd::NoLink(why) => format!("no link: {why}"),
        }
    }

    pub fn describe(&self) -> String {
        let mut s = self.steps.join(" -> ");
        if !s.is_empty() {
            s.push_str(" -> ");
        }
        s.push_str(&self.end_text());
        s
    }
}

fn name_of(e: &Value) -> &str {
    e.get("Name").and_then(|v| v.as_str()).unwrap_or("")
}

fn object_path(v: &Value) -> Option<&str> {
    v.get("ObjectPath").and_then(|p| p.as_str())
}

/// Index of a reference into `exports` when it points into `own_pkg`.
fn local_index(exports: &[Value], own_pkg: &str, reference: &Value) -> Option<usize> {
    let (pkg, idx) = split_object_path(object_path(reference)?);
    if pkg != own_pkg {
        return None;
    }
    idx.filter(|i| *i < exports.len())
}

/// Package of an export array, from its first export's `Outer` (the level or asset it lives in).
pub fn own_package(exports: &[Value], actor: usize) -> String {
    exports
        .get(actor)
        .and_then(|e| e.get("Outer"))
        .and_then(object_path)
        .map(|p| split_object_path(p).0.to_string())
        .or_else(|| exports.iter().find_map(|e| e.get("Package").and_then(|p| p.as_str()).map(str::to_string)))
        .unwrap_or_default()
}

fn archetype_field(props: &Value) -> Option<String> {
    for key in ARCHETYPE_KEYS {
        let Some(v) = props.get(*key) else { continue };
        if let Some(s) = v.as_str() {
            return Some(s.to_string());
        }
        if let Some(p) = object_path(v).or_else(|| v.get("AssetPathName").and_then(|n| n.as_str())) {
            return Some(p.to_string());
        }
    }
    None
}

/// Follow a spawner's archetype from its component export (see the module docs).
pub fn archetype_chain(exports: &[Value], actor: usize, packages: &mut Packages) -> ArchetypeChain {
    let mut steps = Vec::new();
    let Some(e) = exports.get(actor) else {
        return ArchetypeChain { steps, end: ChainEnd::NoLink("no export".into()) };
    };
    let empty = Value::Null;
    let props = e.get("Properties").unwrap_or(&empty);
    if let Some(raw) = archetype_field(props) {
        return ArchetypeChain { steps, end: ChainEnd::Archetype(raw) };
    }
    let own = own_package(exports, actor);
    let mut comp = None;
    if let Some(c) = props.get("Cluster") {
        match local_index(exports, &own, c) {
            Some(ci) => {
                steps.push(format!("Cluster {}", name_of(&exports[ci])));
                let cprops = exports[ci].get("Properties").unwrap_or(&empty);
                if let Some(raw) = archetype_field(cprops) {
                    return ArchetypeChain { steps, end: ChainEnd::Archetype(raw) };
                }
                comp = cprops.get("AgentSource").and_then(|a| local_index(exports, &own, a));
                if comp.is_none() {
                    return ArchetypeChain { steps, end: ChainEnd::NoLink("cluster has no AgentSource".into()) };
                }
            }
            None => return ArchetypeChain { steps, end: ChainEnd::NoLink("Cluster is empty or outside this level".into()) },
        }
    } else if let Some(a) = props.get("AgentSource") {
        comp = local_index(exports, &own, a);
    } else if let Some(s) = props.get("ParticipantSlot") {
        comp = local_index(exports, &own, s);
    }
    let Some(ci) = comp else {
        return ArchetypeChain { steps, end: ChainEnd::NoLink("no Cluster, AgentSource or ParticipantSlot".into()) };
    };
    let end = component_chain(exports, ci, &own, packages, &mut steps, 0);
    ArchetypeChain { steps, end }
}

fn component_chain(
    exports: &[Value],
    index: usize,
    own: &str,
    packages: &mut Packages,
    steps: &mut Vec<String>,
    depth: usize,
) -> ChainEnd {
    if depth > 12 {
        return ChainEnd::NoLink("class default chain deeper than 12".into());
    }
    let e = &exports[index];
    let ty = e.get("Type").and_then(|v| v.as_str()).unwrap_or("");
    steps.push(if depth == 0 { format!("{} {}", ty, name_of(e)) } else { format!("class default {own}.{index}") });
    let empty = Value::Null;
    let props = e.get("Properties").unwrap_or(&empty);
    if let Some(raw) = archetype_field(props) {
        return ChainEnd::Archetype(raw);
    }
    if let Some(group) = props.get("Participant").and_then(|p| p.get("SpawnableGroup")) {
        let Some(path) = object_path(group) else {
            return ChainEnd::NoLink("Participant.SpawnableGroup is empty".into());
        };
        let (pkg, idx) = split_object_path(path);
        steps.push(format!("SpawnableGroup {pkg}"));
        return match packages.get(pkg) {
            None if pkg.starts_with("/Script/") => ChainEnd::Native(pkg.to_string()),
            None => ChainEnd::NotDumped { what: "spawnable group", package: pkg.to_string(), export: idx },
            Some(group_exports) => {
                let found = idx.and_then(|i| group_exports.get(i)).and_then(|g| g.get("Properties")).and_then(archetype_field);
                match found {
                    Some(raw) => ChainEnd::Archetype(raw),
                    None => ChainEnd::NotRead { what: "spawnable group", package: pkg.to_string() },
                }
            }
        };
    }
    if depth == 0 {
        if let Some(src) = props.get("AgentSource").and_then(|a| local_index(exports, own, a)) {
            if src != index {
                return component_chain(exports, src, own, packages, steps, depth);
            }
        }
    }
    let Some(tpath) = e.get("Template").and_then(object_path) else {
        return ChainEnd::Native(format!("{ty} (no Template; class default is native)"));
    };
    let (pkg, idx) = split_object_path(tpath);
    if pkg.starts_with("/Script/") {
        return ChainEnd::Native(pkg.to_string());
    }
    let Some(i) = idx else {
        return ChainEnd::NoLink(format!("Template has no export index: {tpath}"));
    };
    match packages.get(pkg).map(|v| v.clone()) {
        None => ChainEnd::NotDumped { what: "class default", package: pkg.to_string(), export: Some(i) },
        Some(class_exports) if i < class_exports.len() => {
            component_chain(&class_exports, i, pkg, packages, steps, depth + 1)
        }
        Some(_) => ChainEnd::NoLink(format!("Template export {i} past the end of {pkg}")),
    }
}

/// Where one attribute value came from.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrSource {
    /// `InitialSettings` in the class asset.
    Class,
    /// `InitialSettings` in the per-variant asset (overrides the class asset).
    Variant,
    /// The `DA_AttributeType_*` asset's `DefaultValue`.
    TypeDefault(String),
    /// The field takes its type asset's `DefaultValue`, and that asset is not in the export.
    TypeAssetNotDumped(String),
    /// Not in the cooked data at all: [`NATIVE_DEFAULT`], with what is missing.
    Native(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttrValue {
    pub field: String,
    pub desired: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub source: AttrSource,
    /// Parts the asset leaves to the struct's native default (e.g. `MinValue` when only `MaxValue` is set).
    pub native_parts: Vec<&'static str>,
}

impl AttrValue {
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = self.desired {
            parts.push(format!("{v}"));
        }
        if self.min.is_some() || self.max.is_some() {
            let f = |o: Option<f64>| o.map(|v| v.to_string()).unwrap_or_else(|| NATIVE_DEFAULT.to_string());
            parts.push(format!("range {}..{}", f(self.min), f(self.max)));
        }
        let value = if parts.is_empty() { "-".to_string() } else { parts.join(" ") };
        let src = match &self.source {
            AttrSource::Class => "class asset".to_string(),
            AttrSource::Variant => "variant asset".to_string(),
            AttrSource::TypeDefault(p) => format!("type DefaultValue {p}"),
            AttrSource::TypeAssetNotDumped(p) => format!("type asset not in export: {p}"),
            AttrSource::Native(why) => format!("{NATIVE_DEFAULT} ({why})"),
        };
        let mut s = format!("{} = {value} | {src}", self.field);
        for p in &self.native_parts {
            s.push_str(&format!(" | {p}: {NATIVE_DEFAULT}"));
        }
        s
    }
}

#[derive(Debug, Clone)]
pub struct AttributeRead {
    pub archetype: String,
    pub class_asset: Option<PathBuf>,
    pub variant_asset: Option<PathBuf>,
    pub values: Vec<AttrValue>,
}

/// First `<name>.json` under `root` (sorted walk, so the pick is fixed).
pub fn find_asset(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    let file = format!("{name}.json");
    let mut hits = Vec::new();
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().map(|n| n == file.as_str()).unwrap_or(false) {
                hits.push(p);
            }
        }
    }
    hits.sort();
    hits.into_iter().next()
}

fn asset_props(path: &Path) -> Option<serde_json::Map<String, Value>> {
    let v: Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    v.as_array()?.first()?.get("Properties")?.as_object().cloned()
}

/// Values for one archetype id (`<Class>` or `<Class>_<Variant>`), see the module docs.
pub fn resolve_attributes(packages: &mut Packages, archetype: &str) -> AttributeRead {
    let mut out = AttributeRead { archetype: archetype.to_string(), class_asset: None, variant_asset: None, values: Vec::new() };
    let Some(root) = packages.root().map(Path::to_path_buf) else { return out };
    let class = archetype.split('_').next().unwrap_or(archetype);
    out.class_asset = find_asset(&root, &format!("DA_AIAttributeValues_{class}"));
    if archetype != class {
        out.variant_asset = find_asset(&root, &format!("DA_AIAttributeValues_{archetype}"));
    }
    let class_props = out.class_asset.as_deref().and_then(asset_props).unwrap_or_default();
    let variant_props = out.variant_asset.as_deref().and_then(asset_props).unwrap_or_default();
    let mut fields: Vec<&String> = class_props.keys().chain(variant_props.keys()).collect();
    fields.sort();
    fields.dedup();
    for field in fields {
        let (v, from_variant) = match variant_props.get(field) {
            Some(v) => (v, true),
            None => (&class_props[field], false),
        };
        // A list field (e.g. extra types with initial values) holds one attribute per entry, named by its type.
        if let Some(list) = v.as_array() {
            for item in list {
                let ty = type_ref(item).map(|p| split_object_path(p).0.rsplit('/').next().unwrap_or("").to_string());
                let name = format!("{field}[{}]", ty.unwrap_or_default());
                out.values.push(resolve_field(packages, &name, item, from_variant));
            }
            continue;
        }
        out.values.push(resolve_field(packages, field, v, from_variant));
    }
    out
}

/// The attribute type reference: cooked fields name it `AttributeType` or `Type`.
fn type_ref(v: &Value) -> Option<&str> {
    v.get("AttributeType").or_else(|| v.get("Type")).and_then(object_path)
}

fn resolve_field(packages: &mut Packages, field: &str, v: &Value, from_variant: bool) -> AttrValue {
    let mut a = AttrValue { field: field.to_string(), desired: None, min: None, max: None, source: AttrSource::Native(String::new()), native_parts: Vec::new() };
    let empty = Value::Null;
    let s = v.get("InitialSettings").unwrap_or(&empty);
    let own_value = s.get("bUseTypeDefaultValue").and_then(|b| b.as_bool()) == Some(false);
    if own_value {
        a.desired = s.get("DesiredValue").and_then(|n| n.as_f64());
        a.min = s.get("MinValue").and_then(|n| n.as_f64());
        a.max = s.get("MaxValue").and_then(|n| n.as_f64());
        if a.desired.is_none() && a.min.is_none() && a.max.is_none() {
            a.source = AttrSource::Native("bUseTypeDefaultValue false, no value in the asset".into());
            return a;
        }
        if a.min.is_none() && a.max.is_some() {
            a.native_parts.push("MinValue");
        }
        if a.max.is_none() && a.min.is_some() {
            a.native_parts.push("MaxValue");
        }
        a.source = if from_variant { AttrSource::Variant } else { AttrSource::Class };
        return a;
    }
    let Some(tpath) = type_ref(v) else {
        a.source = AttrSource::Native("no AttributeType / Type on the field".into());
        return a;
    };
    let (pkg, idx) = split_object_path(tpath);
    let pkg = pkg.to_string();
    match packages.get(&pkg) {
        None => a.source = AttrSource::TypeAssetNotDumped(pkg),
        Some(t) => {
            let dv = t.get(idx.unwrap_or(0)).and_then(|e| e.get("Properties")).and_then(|p| p.get("DefaultValue"));
            match dv.and_then(|n| n.as_f64()) {
                Some(n) => {
                    a.desired = Some(n);
                    a.source = AttrSource::TypeDefault(pkg);
                }
                None => a.source = AttrSource::Native(format!("DefaultValue not in {pkg}")),
            }
        }
    }
    a
}
