//! Scene-graph comparison for assets a mod replaces wholesale.
//!
//! Most mods ship a *sparse* EXML patch: a handful of properties the game
//! merges into the vanilla asset. Those are covered by [`super::exml`]. A scene
//! mod is different -- it ships the whole `.SCENE.MBIN` back, so every node in
//! the file is the mod's version of that node, whether the author meant to
//! touch it or not.
//!
//! That makes a class of bug invisible to the property-level checker. An author
//! who re-exports a scene through a 3D tool can have the tool rebake or reorder
//! transforms; the geometry still renders correctly, because the offset was
//! pushed down into the children, but a *locator* the engine reads as an anchor
//! has quietly moved. Nothing is malformed, nothing conflicts, and the mod
//! looks perfectly healthy right up until the game places something 24 units
//! from where it belongs.
//!
//! The check is therefore positional, not textual: walk both scene graphs,
//! compose each node's world transform, and compare. A node that exists in both
//! and has moved is the finding. Nodes added or removed are the mod doing its
//! job.
//!
//! **Rotation order.** `TkTransformData` gives Euler angles in degrees with no
//! stated order; this module composes `Rx * Ry * Rz`. Detection does not depend
//! on that choice, because both sides of a comparison use the same convention
//! and an identical transform composes identically either way. It only affects
//! the reported distance for a node underneath a rotated parent.

use std::path::Path;

use indexmap::IndexMap;

/// Template of a scene graph root. Vanilla decompiles carry the `c` prefix;
/// AMUMSS output frequently drops it, and both forms load, so accept either.
pub const SCENE_TEMPLATES: [&str; 2] = ["cTkSceneNodeData", "TkSceneNodeData"];

/// World-space distance, in game units, below which two nodes count as being
/// in the same place.
///
/// Re-exporting a scene perturbs floats in the sixth decimal (`65.861860`
/// becomes `65.8618546`); this sits far above that noise and far below any
/// move that could be deliberate.
pub const MOVE_EPSILON: f64 = 0.01;

/// Same idea for the rotation/scale part of the matrix, which is unitless.
pub const BASIS_EPSILON: f64 = 1e-4;

/// Row-major 4x4.
pub type Matrix = [f64; 16];

pub const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

/// One node of a scene graph, with its transform already composed.
#[derive(Debug, Clone)]
pub struct SceneNode {
    /// Path from the root, root excluded, e.g. `FREIGHTERBASE/BaseBuildingData`.
    ///
    /// The root is excluded so variants of one asset (`HANGAR` versus
    /// `HANGARPIRATE`) line up despite their differing root names.
    pub path: String,
    /// LOCATOR / MESH / REFERENCE / JOINT / ...
    pub kind: String,
    pub world: Matrix,
    /// `SCENEGRAPH` attribute of a REFERENCE node, so an added reference can be
    /// reported as what it pulls in rather than as a bare node name.
    pub scenegraph: Option<String>,
}

impl SceneNode {
    pub fn position(&self) -> (f64, f64, f64) {
        (self.world[3], self.world[7], self.world[11])
    }
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub root_name: String,
    pub nodes: IndexMap<String, SceneNode>,
    pub error: Option<String>,
}

/// A node present in both scenes whose world transform differs.
#[derive(Debug, Clone)]
pub struct MovedNode {
    pub path: String,
    pub kind: String,
    pub before: (f64, f64, f64),
    pub after: (f64, f64, f64),
    /// True when only the rotation/scale basis changed and the origin held.
    pub rotated_only: bool,
    /// True when this node moved but everything underneath it stayed put.
    ///
    /// This is the signature of a rebake rather than an edit. Moving a parent
    /// on purpose carries its contents along; an exporter that pushes a
    /// parent's offset down into its children leaves the geometry exactly
    /// where it was and moves only the parent. Nothing looks wrong in game
    /// until something reads that node's position directly.
    pub contents_held: bool,
}

impl MovedNode {
    pub fn distance(&self) -> f64 {
        let (ax, ay, az) = self.before;
        let (bx, by, bz) = self.after;
        ((ax - bx).powi(2) + (ay - by).powi(2) + (az - bz).powi(2)).sqrt()
    }

    /// The single axis a pure axis-aligned move happened on, else `""`.
    pub fn axis(&self) -> &'static str {
        let deltas = [
            (self.after.0 - self.before.0).abs(),
            (self.after.1 - self.before.1).abs(),
            (self.after.2 - self.before.2).abs(),
        ];
        let moved: Vec<usize> = (0..3).filter(|&i| deltas[i] > MOVE_EPSILON).collect();
        match moved.as_slice() {
            [0] => "X",
            [1] => "Y",
            [2] => "Z",
            _ => "",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SceneDiff {
    pub added: Vec<SceneNode>,
    pub removed: Vec<SceneNode>,
    pub moved: Vec<MovedNode>,
}

impl SceneDiff {
    /// True when nothing that exists in both scenes has shifted.
    pub fn clean(&self) -> bool {
        self.moved.is_empty()
    }
}

fn matmul(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0f64; 16];
    for r in 0..4 {
        for c in 0..4 {
            out[r * 4 + c] = a[r * 4] * b[c]
                + a[r * 4 + 1] * b[4 + c]
                + a[r * 4 + 2] * b[8 + c]
                + a[r * 4 + 3] * b[12 + c];
        }
    }
    out
}

/// Compose `T * Rx * Ry * Rz * S` for one node.
#[allow(clippy::too_many_arguments)]
pub fn local_matrix(
    tx: f64,
    ty: f64,
    tz: f64,
    rx: f64,
    ry: f64,
    rz: f64,
    sx: f64,
    sy: f64,
    sz: f64,
) -> Matrix {
    let (cx, sinx) = (rx.to_radians().cos(), rx.to_radians().sin());
    let (cy, siny) = (ry.to_radians().cos(), ry.to_radians().sin());
    let (cz, sinz) = (rz.to_radians().cos(), rz.to_radians().sin());

    // Rx * Ry * Rz, written out rather than multiplied three times.
    let m00 = cy * cz;
    let m01 = -cy * sinz;
    let m02 = siny;
    let m10 = sinx * siny * cz + cx * sinz;
    let m11 = -sinx * siny * sinz + cx * cz;
    let m12 = -sinx * cy;
    let m20 = -cx * siny * cz + sinx * sinz;
    let m21 = cx * siny * sinz + sinx * cz;
    let m22 = cx * cy;

    [
        m00 * sx, m01 * sy, m02 * sz, tx, //
        m10 * sx, m11 * sy, m12 * sz, ty, //
        m20 * sx, m21 * sy, m22 * sz, tz, //
        0.0, 0.0, 0.0, 1.0,
    ]
}

/// Direct `<Property name="...">` children, by name.
fn props<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
) -> IndexMap<String, roxmltree::Node<'a, 'input>> {
    let mut out = IndexMap::new();
    for child in node.children().filter(|c| c.is_element()) {
        if !child.has_tag_name("Property") {
            continue;
        }
        if let Some(name) = child.attribute("name") {
            out.entry(name.to_string()).or_insert(child);
        }
    }
    out
}

fn float_of(node: Option<&roxmltree::Node>, default: f64) -> f64 {
    node.and_then(|n| n.attribute("value"))
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(default)
}

fn transform_of(p: &IndexMap<String, roxmltree::Node>) -> Matrix {
    let Some(t) = p.get("Transform") else {
        return IDENTITY;
    };
    let d = props(*t);
    local_matrix(
        float_of(d.get("TransX"), 0.0),
        float_of(d.get("TransY"), 0.0),
        float_of(d.get("TransZ"), 0.0),
        float_of(d.get("RotX"), 0.0),
        float_of(d.get("RotY"), 0.0),
        float_of(d.get("RotZ"), 0.0),
        float_of(d.get("ScaleX"), 1.0),
        float_of(d.get("ScaleY"), 1.0),
        float_of(d.get("ScaleZ"), 1.0),
    )
}

/// The `SCENEGRAPH` attribute value, for REFERENCE nodes.
fn scenegraph_of(p: &IndexMap<String, roxmltree::Node>) -> Option<String> {
    let attrs = p.get("Attributes")?;
    for entry in attrs.children().filter(|c| c.is_element()) {
        let d = props(entry);
        let is_scenegraph = d
            .get("Name")
            .and_then(|n| n.attribute("value"))
            .map(|v| v == "SCENEGRAPH")
            .unwrap_or(false);
        if is_scenegraph {
            return d
                .get("Value")
                .and_then(|n| n.attribute("value"))
                .map(str::to_string);
        }
    }
    None
}

/// Scene roots carry a full asset path; nodes carry a bare name.
fn leaf_name(raw: &str) -> String {
    raw.replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

fn walk(
    node: roxmltree::Node,
    prefix: &str,
    parent: &Matrix,
    out: &mut IndexMap<String, SceneNode>,
    is_root: bool,
) {
    let p = props(node);
    let raw = p
        .get("Name")
        .and_then(|n| n.attribute("value"))
        .unwrap_or("");
    let name = leaf_name(raw);
    let kind = p
        .get("Type")
        .and_then(|n| n.attribute("value"))
        .unwrap_or("")
        .to_string();

    let world = matmul(parent, &transform_of(&p));
    // The root is the asset itself; excluding it from the key lets sibling
    // variants of one asset be compared directly.
    let path = if is_root {
        String::new()
    } else if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    };

    // Sibling nodes may share a name (several "Collision" children under one
    // parent). Disambiguate only on collision, so the common case stays stable
    // regardless of how many siblings there happen to be.
    let mut key = path.clone();
    if out.contains_key(&key) {
        let mut n = 2;
        while out.contains_key(&format!("{path}#{n}")) {
            n += 1;
        }
        key = format!("{path}#{n}");
    }

    out.insert(
        key.clone(),
        SceneNode {
            path: key,
            kind,
            world,
            scenegraph: scenegraph_of(&p),
        },
    );

    if let Some(children) = p.get("Children") {
        for child in children.children().filter(|c| c.is_element()) {
            if child.has_tag_name("Property") {
                walk(child, &path, &world, out, false);
            }
        }
    }
}

/// Read a decompiled scene file into world-space nodes.
///
/// Returns a [`Scene`] whose `error` is set when the file is not a scene graph
/// or cannot be parsed; callers treat that as "nothing to compare" rather than
/// as a finding, since [`super::analyze`] already reports unreadable files in
/// its own right.
pub fn parse(path: &Path) -> Scene {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            return Scene {
                error: Some(err.to_string()),
                ..Default::default()
            }
        }
    };
    parse_str(&text)
}

pub fn parse_str(text: &str) -> Scene {
    let parsed = match roxmltree::Document::parse(text) {
        Ok(doc) => doc,
        Err(err) => {
            return Scene {
                error: Some(format!("XML parse error: {err}")),
                ..Default::default()
            }
        }
    };
    let root = parsed.root_element();
    let template = root.attribute("template").unwrap_or("");
    if !SCENE_TEMPLATES.contains(&template) {
        return Scene {
            error: Some(format!("not a scene graph ({template})")),
            ..Default::default()
        };
    }

    let mut nodes = IndexMap::new();
    walk(root, "", &IDENTITY, &mut nodes, true);
    let root_name = props(root)
        .get("Name")
        .and_then(|n| n.attribute("value"))
        .unwrap_or("")
        .to_string();
    Scene {
        root_name,
        nodes,
        error: None,
    }
}

/// True when `path` moved but nothing underneath it did.
///
/// Only counts when the node actually has descendants present in both copies:
/// a leaf that moves has genuinely taken its geometry with it, which is what an
/// intentional reposition looks like.
fn contents_held(
    path: &str,
    before: &Scene,
    after: &Scene,
    shifted: &std::collections::HashSet<String>,
) -> bool {
    let prefix = format!("{path}/");
    let mut any = false;
    for key in before.nodes.keys() {
        if !key.starts_with(&prefix) || !after.nodes.contains_key(key) {
            continue;
        }
        any = true;
        if shifted.contains(key) {
            return false;
        }
    }
    any
}

fn dist(a: (f64, f64, f64), b: (f64, f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt()
}

/// Compare two scene graphs node by node in world space.
///
/// `before` is the reference (vanilla); `after` is the copy under suspicion.
pub fn diff(before: &Scene, after: &Scene) -> SceneDiff {
    let mut out = SceneDiff::default();

    for (key, node) in &after.nodes {
        if !before.nodes.contains_key(key) {
            out.added.push(node.clone());
        }
    }
    for (key, node) in &before.nodes {
        let Some(other) = after.nodes.get(key) else {
            out.removed.push(node.clone());
            continue;
        };
        let shifted = dist(node.position(), other.position()) > MOVE_EPSILON;
        let basis = [0usize, 1, 2, 4, 5, 6, 8, 9, 10]
            .iter()
            .any(|&i| (node.world[i] - other.world[i]).abs() > BASIS_EPSILON);
        if shifted || basis {
            out.moved.push(MovedNode {
                path: key.clone(),
                kind: node.kind.clone(),
                before: node.position(),
                after: other.position(),
                rotated_only: basis && !shifted,
                contents_held: false,
            });
        }
    }

    // A node whose basis turned but whose origin held has not displaced
    // anything, so it is not the rebake this flag is for.
    let shifted: std::collections::HashSet<String> = out
        .moved
        .iter()
        .filter(|m| !m.rotated_only)
        .map(|m| m.path.clone())
        .collect();
    for entry in &mut out.moved {
        entry.contents_held =
            !entry.rotated_only && contents_held(&entry.path, before, after, &shifted);
    }

    out.moved.sort_by(|a, b| {
        b.distance()
            .partial_cmp(&a.distance())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
    });
    out.added.sort_by(|a, b| a.path.cmp(&b.path));
    out.removed.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
        <!--File created using MBINCompiler version (7.03.2.2)-->\n";

    fn xf(x: f64, y: f64, z: f64, ry: f64) -> String {
        format!(
            "<Property name=\"Transform\" value=\"TkTransformData\">\
             <Property name=\"TransX\" value=\"{x:.6}\" />\
             <Property name=\"TransY\" value=\"{y:.6}\" />\
             <Property name=\"TransZ\" value=\"{z:.6}\" />\
             <Property name=\"RotX\" value=\"0.000000\" />\
             <Property name=\"RotY\" value=\"{ry:.6}\" />\
             <Property name=\"RotZ\" value=\"0.000000\" />\
             <Property name=\"ScaleX\" value=\"1.000000\" />\
             <Property name=\"ScaleY\" value=\"1.000000\" />\
             <Property name=\"ScaleZ\" value=\"1.000000\" />\
             </Property>"
        )
    }

    fn node(name: &str, kind: &str, transform: &str, children: &str) -> String {
        let body = if children.is_empty() {
            "<Property name=\"Children\" />".to_string()
        } else {
            format!("<Property name=\"Children\">{children}</Property>")
        };
        format!(
            "<Property name=\"Children\" value=\"TkSceneNodeData\">\
             <Property name=\"Name\" value=\"{name}\" />\
             <Property name=\"Type\" value=\"{kind}\" />\
             {transform}<Property name=\"Attributes\" />{body}</Property>"
        )
    }

    fn scene(children: &str, root: &str) -> String {
        format!(
            "{HEADER}<Data template=\"cTkSceneNodeData\">\
             <Property name=\"Name\" value=\"MODELS\\COMMON\\{root}\" />\
             <Property name=\"Type\" value=\"MODEL\" />{}\
             <Property name=\"Children\">{children}</Property></Data>",
            xf(0.0, 0.0, 0.0, 0.0)
        )
    }

    fn pair(before: &str, after: &str) -> SceneDiff {
        let a = parse_str(&scene(before, "HANGAR"));
        let b = parse_str(&scene(after, "HANGAR"));
        assert!(a.error.is_none(), "{:?}", a.error);
        assert!(b.error.is_none(), "{:?}", b.error);
        diff(&a, &b)
    }

    #[test]
    fn root_is_excluded_from_node_keys() {
        let a = parse_str(&scene(&node("BASE", "LOCATOR", &xf(0.0, 0.0, 0.0, 0.0), ""), "HANGAR"));
        let b = parse_str(&scene(
            &node("BASE", "LOCATOR", &xf(0.0, 0.0, 0.0, 0.0), ""),
            "HANGARPIRATE",
        ));
        assert!(a.nodes.contains_key("BASE"));
        let ka: Vec<_> = a.nodes.keys().collect();
        let kb: Vec<_> = b.nodes.keys().collect();
        assert_eq!(ka, kb);
    }

    #[test]
    fn world_position_composes_through_parents() {
        let tree = node(
            "BASE",
            "LOCATOR",
            &xf(0.0, 24.0, 0.0, 0.0),
            &node("Child", "MESH", &xf(0.0, 1.5, 0.0, 0.0), ""),
        );
        let parsed = parse_str(&scene(&tree, "HANGAR"));
        assert!((parsed.nodes["BASE"].position().1 - 24.0).abs() < 1e-5);
        assert!((parsed.nodes["BASE/Child"].position().1 - 25.5).abs() < 1e-5);
    }

    #[test]
    fn parent_rotation_moves_the_child() {
        // Naively summing translations would miss this.
        let tree = node(
            "BASE",
            "LOCATOR",
            &xf(0.0, 0.0, 0.0, 180.0),
            &node("Child", "MESH", &xf(0.0, 0.0, 10.0, 0.0), ""),
        );
        let parsed = parse_str(&scene(&tree, "HANGAR"));
        assert!((parsed.nodes["BASE/Child"].position().2 + 10.0).abs() < 1e-4);
    }

    #[test]
    fn siblings_sharing_a_name_stay_distinct() {
        let tree = format!(
            "{}{}",
            node("Collision", "COLLISION", &xf(1.0, 0.0, 0.0, 0.0), ""),
            node("Collision", "COLLISION", &xf(2.0, 0.0, 0.0, 0.0), "")
        );
        let parsed = parse_str(&scene(&tree, "HANGAR"));
        assert!(parsed.nodes.contains_key("Collision"));
        assert!(parsed.nodes.contains_key("Collision#2"));
    }

    #[test]
    fn non_scene_file_is_rejected() {
        let text = format!("{HEADER}<Data template=\"cGcRewardTable\"></Data>");
        assert!(parse_str(&text).error.is_some());
    }

    #[test]
    fn reference_target_is_captured() {
        let body = format!(
            "<Property name=\"Children\" value=\"TkSceneNodeData\">\
             <Property name=\"Name\" value=\"Terminal\" />\
             <Property name=\"Type\" value=\"REFERENCE\" />{}\
             <Property name=\"Attributes\">\
             <Property name=\"Attributes\" value=\"TkSceneNodeAttributeData\">\
             <Property name=\"Name\" value=\"SCENEGRAPH\" />\
             <Property name=\"Value\" value=\"MODELS\\X\\TERMINAL.SCENE.MBIN\" />\
             </Property></Property>\
             <Property name=\"Children\" /></Property>",
            xf(0.0, 0.0, 0.0, 0.0)
        );
        let parsed = parse_str(&scene(&body, "HANGAR"));
        assert_eq!(
            parsed.nodes["Terminal"].scenegraph.as_deref(),
            Some("MODELS\\X\\TERMINAL.SCENE.MBIN")
        );
    }

    #[test]
    fn identical_scenes_are_clean() {
        let tree = node("BASE", "LOCATOR", &xf(0.0, 24.0, 0.0, 0.0), "");
        assert!(pair(&tree, &tree).clean());
    }

    #[test]
    fn float_noise_from_a_re_export_is_not_a_move() {
        // 65.861860 becoming 65.8618546 is not a finding.
        let before = node("Pad", "MESH", &xf(0.0, 0.0, 65.861_860, 0.0), "");
        let after = node("Pad", "MESH", &xf(0.0, 0.0, 65.861_854_6, 0.0), "");
        assert!(pair(&before, &after).clean());
    }

    #[test]
    fn added_and_removed_nodes_are_counted_not_flagged() {
        let before = node("BASE", "LOCATOR", &xf(0.0, 0.0, 0.0, 0.0), "");
        let after = format!(
            "{before}{}",
            node("Terminal", "REFERENCE", &xf(22.0, 0.0, 0.0, 0.0), "")
        );
        let d = pair(&before, &after);
        assert!(d.clean());
        assert_eq!(d.added.iter().map(|n| n.path.as_str()).collect::<Vec<_>>(), ["Terminal"]);
        assert!(d.removed.is_empty());
    }

    #[test]
    fn rebake_moves_the_parent_but_not_its_contents() {
        // The real bug: locator drops 24.28, children compensate exactly.
        let before = node(
            "FREIGHTERBASE",
            "LOCATOR",
            &xf(0.0, 24.280_840, 0.0, 0.0),
            &format!(
                "{}{}",
                node("RefBridge", "REFERENCE", &xf(0.0, 3.949_570, 0.0, 0.0), ""),
                node("BaseBuildingData", "LOCATOR", &xf(0.0, 1.739_448, 0.0, 0.0), "")
            ),
        );
        let after = node(
            "FREIGHTERBASE",
            "LOCATOR",
            &xf(0.0, 0.0, 0.0, 0.0),
            &format!(
                "{}{}",
                node("RefBridge", "REFERENCE", &xf(0.0, 28.230_410, 0.0, 0.0), ""),
                node("BaseBuildingData", "LOCATOR", &xf(0.0, 26.020_288, 0.0, 0.0), "")
            ),
        );
        let d = pair(&before, &after);
        assert_eq!(
            d.moved.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(),
            ["FREIGHTERBASE"]
        );
        assert!((d.moved[0].distance() - 24.280_840).abs() < 1e-4);
        assert_eq!(d.moved[0].axis(), "Y");
        assert!(d.moved[0].contents_held);
    }

    #[test]
    fn deliberate_move_carries_its_contents() {
        // A mod repositioning something on purpose must not be flagged.
        let before = node(
            "Teleporter",
            "LOCATOR",
            &xf(0.0, 0.0, 0.0, 0.0),
            &node("Mesh", "MESH", &xf(0.0, 0.0, 0.0, 0.0), ""),
        );
        let after = node(
            "Teleporter",
            "LOCATOR",
            &xf(68.0, 0.0, 0.0, 0.0),
            &node("Mesh", "MESH", &xf(0.0, 0.0, 0.0, 0.0), ""),
        );
        let d = pair(&before, &after);
        let mut paths: Vec<_> = d.moved.iter().map(|m| m.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, ["Teleporter", "Teleporter/Mesh"]);
        assert!(!d.moved.iter().any(|m| m.contents_held));
    }

    #[test]
    fn moved_leaf_is_never_a_rebake() {
        let before = node("Terminal", "REFERENCE", &xf(0.0, 0.0, 0.0, 0.0), "");
        let after = node("Terminal", "REFERENCE", &xf(68.523, 0.0, 0.0, 0.0), "");
        let d = pair(&before, &after);
        assert_eq!(d.moved.len(), 1);
        assert!(!d.moved[0].contents_held);
    }

    #[test]
    fn rotation_without_displacement_is_not_a_rebake() {
        // A turned basis displaces nothing, so it is not the bug class. The
        // child sits at the parent's origin, so it does not travel, but it
        // inherits the turned basis -- both are reported, neither as a rebake.
        let before = node(
            "Pad",
            "LOCATOR",
            &xf(0.0, 0.0, 0.0, 0.0),
            &node("Child", "MESH", &xf(0.0, 0.0, 0.0, 0.0), ""),
        );
        let after = node(
            "Pad",
            "LOCATOR",
            &xf(0.0, 0.0, 0.0, 90.0),
            &node("Child", "MESH", &xf(0.0, 0.0, 0.0, 0.0), ""),
        );
        let d = pair(&before, &after);
        let mut paths: Vec<_> = d.moved.iter().map(|m| m.path.as_str()).collect();
        paths.sort();
        assert_eq!(paths, ["Pad", "Pad/Child"]);
        assert!(d.moved.iter().all(|m| m.rotated_only));
        assert!(!d.moved.iter().any(|m| m.contents_held));
    }
}
