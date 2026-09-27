//! Serialisation of a [`Report`] into the `--json` shape.
//!
//! This is the contract the front end reads, and the thing
//! `tools/compare_engines.py` diffs against the Python's `render_json`. Field
//! names, ordering and null-vs-absent all have to match, so the structure here
//! deliberately mirrors `nmscc/report.py` line for line rather than deriving
//! `Serialize` on the model types.

use serde_json::{json, Map, Value};

use super::model::{Conflict, Report, SceneDrift};

fn conflict_json(conflict: &Conflict) -> Value {
    let mut unique_counts = Map::new();
    for (name, count) in &conflict.unique_counts {
        unique_counts.insert(name.clone(), json!(count));
    }

    let clashes: Vec<Value> = conflict
        .clashes
        .iter()
        .map(|clash| {
            let mut values = Map::new();
            for (name, value) in &clash.values {
                values.insert(
                    name.clone(),
                    match value {
                        Some(v) => Value::String(v.clone()),
                        None => Value::Null,
                    },
                );
            }
            json!({
                "path": clash.path,
                "values": values,
                "attributed": clash.attributed,
            })
        })
        .collect();

    let mut edit_counts = Map::new();
    for (name, count) in &conflict.edit_counts {
        edit_counts.insert(name.clone(), json!(count));
    }

    json!({
        "target": conflict.target,
        "mods": conflict.mods,
        "severity": conflict.severity.map(|s| s.as_str()),
        "kind": conflict.kind,
        "benign": conflict.benign,
        "mergeable": conflict.mergeable,
        "overlap": conflict.overlap,
        "edit_counts": edit_counts,
        "summary": conflict.summary,
        "predicted_winner": conflict.predicted_winner,
        "unique_counts": unique_counts,
        "declared_only": conflict.declared_only,
        "notes": conflict.notes,
        "clashes": clashes,
    })
}

fn drift_json(drift: &SceneDrift) -> Value {
    let moved: Vec<Value> = drift
        .moved
        .iter()
        .map(|m| {
            json!({
                "path": m.path,
                "kind": m.kind,
                "before": [m.before.0, m.before.1, m.before.2],
                "after": [m.after.0, m.after.1, m.after.2],
                "distance": (m.distance() * 1e6).round() / 1e6,
                "axis": m.axis(),
                "contents_held": m.contents_held,
                "rotated_only": m.rotated_only,
            })
        })
        .collect();

    json!({
        "mod": drift.mod_name,
        "rel_path": drift.rel_path,
        "target": drift.target,
        "severity": drift.severity.as_str(),
        "reference": drift.reference,
        "added": drift.added,
        "removed": drift.removed,
        "moved": moved,
    })
}

/// Serialise the report for the front end and for engine comparison.
pub fn to_json(report: &Report) -> Value {
    let mods: Vec<Value> = report
        .mods
        .iter()
        .map(|m| {
            json!({
                "name": m.name,
                "root": m.root,
                "files": m.files.len(),
                "targets": m.targets().iter().collect::<Vec<_>>(),
                "declared_targets": m.declared_targets.iter().collect::<Vec<_>>(),
                "amumss_version": m.amumss_version,
                "source": m.source,
                "disabled": m.disabled,
                "max_version": m.max_version().map(|v| v.to_string()),
                "min_version": m.min_version().map(|v| v.to_string()),
                "priority": m.priority,
            })
        })
        .collect();

    let stale: Vec<Value> = report
        .stale
        .iter()
        .map(|f| {
            json!({
                "mod": f.mod_name,
                "version": f.version.to_string(),
                "reference": f.reference.to_string(),
                "file_count": f.file_count,
                "severity": f.severity.as_str(),
                "examples": f.examples,
            })
        })
        .collect();

    let broken: Vec<Value> = report
        .broken
        .iter()
        .map(|b| {
            json!({
                "mod": b.mod_name,
                "rel_path": b.rel_path,
                "error": b.error,
            })
        })
        .collect();

    let loc_clashes: Vec<Value> = report
        .loc_clashes
        .iter()
        .map(|c| json!({"id": c.loc_id, "mods": c.mods}))
        .collect();

    json!({
        "roots": report.roots,
        "winner_rule": report.winner_rule,
        "reference_version": report.reference_version.map(|v| v.to_string()),
        "stats": report.stats,
        "actionable_count": report.actionable().count(),
        "merged_count": report.merged().count(),
        "real_load_order": report.real_load_order,
        "manager": report.manager,
        "disable_all": report.disable_all,
        "disabled": report.disabled,
        "unregistered": report.unregistered,
        "mods": mods,
        "conflicts": report.conflicts.iter().map(conflict_json).collect::<Vec<_>>(),
        "broken": broken,
        "drift": report.drift.iter().map(drift_json).collect::<Vec<_>>(),
        "tools": report.tools,
        "stale": stale,
        "loc_clashes": loc_clashes,
    })
}
