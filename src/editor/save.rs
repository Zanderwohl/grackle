use std::path::{Path, PathBuf};
use bevy::platform::collections::HashMap;
use bevy::prelude::{Vec3, info};
use rusqlite::{Connection, Transaction, params};
use crate::common::skeleton::AnimationState;
use crate::constants::{SCHEMA_VERSION, MAP_BLUEPRINT_EXTENSION, MAP_BACKUP_EXTENSION};
use crate::editor::action::{Action, FeatureData, FeatureDelta, FeatureSnapshot};
use crate::editor::editable::{
    AxisRef, Feature, FeatureId, FeatureTimeline, PointRef,
    create_object_from_type_key,
};
use crate::editor::map_metadata::MapMetadata;

/// Result of loading a blueprint file (features + map metadata).
pub struct LoadedBlueprint {
    pub timeline: FeatureTimeline,
    pub metadata: MapMetadata,
}

pub fn map_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.{}", name, MAP_BLUEPRINT_EXTENSION))
}

fn migrations() -> Vec<(u64, Vec<&'static str>)> {
    vec![
        (1, vec![
            "CREATE TABLE IF NOT EXISTS metadata (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
            "CREATE TABLE IF NOT EXISTS editor_meta (
                id_counter INTEGER NOT NULL,
                cursor     INTEGER NOT NULL
            );",
            "CREATE TABLE IF NOT EXISTS features (
                id          INTEGER PRIMARY KEY,
                type_key    TEXT    NOT NULL,
                order_index INTEGER NOT NULL UNIQUE
            );",
            "CREATE TABLE IF NOT EXISTS feature_parents (
                feature_id INTEGER NOT NULL REFERENCES features(id),
                parent_id INTEGER NOT NULL REFERENCES features(id),
                PRIMARY KEY (feature_id, parent_id)
            );",
            "CREATE TABLE IF NOT EXISTS point_refs (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                owner_feature_id     INTEGER NOT NULL REFERENCES features(id),
                slot                TEXT    NOT NULL,
                reference_feature_id INTEGER REFERENCES features(id),
                point_key           TEXT    NOT NULL DEFAULT '',
                x_mode              TEXT    NOT NULL,
                x_value             REAL    NOT NULL,
                y_mode              TEXT    NOT NULL,
                y_value             REAL    NOT NULL,
                z_mode              TEXT    NOT NULL,
                z_value             REAL    NOT NULL
            );",
            "CREATE TABLE IF NOT EXISTS scalar_fields (
                owner_feature_id INTEGER NOT NULL REFERENCES features(id),
                field_key       TEXT    NOT NULL,
                field_value     REAL    NOT NULL,
                PRIMARY KEY (owner_feature_id, field_key)
            );",
        ]),
        (2, vec![
            "ALTER TABLE editor_meta RENAME COLUMN cursor TO rollback_bar;",
        ]),
        (3, vec![
            "CREATE TABLE IF NOT EXISTS feature_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                subject_feature_id INTEGER NOT NULL REFERENCES features(id),
                order_index INTEGER NOT NULL,
                data_kind TEXT NOT NULL
            );",
            "CREATE TABLE IF NOT EXISTS snapshot_parents (
                snapshot_id INTEGER NOT NULL REFERENCES feature_snapshots(id) ON DELETE CASCADE,
                parent_id INTEGER NOT NULL REFERENCES features(id),
                PRIMARY KEY (snapshot_id, parent_id)
            );",
            "CREATE TABLE IF NOT EXISTS snapshot_point_refs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                snapshot_id INTEGER NOT NULL REFERENCES feature_snapshots(id) ON DELETE CASCADE,
                slot TEXT NOT NULL,
                reference_feature_id INTEGER REFERENCES features(id),
                point_key TEXT NOT NULL DEFAULT '',
                x_mode TEXT NOT NULL,
                x_value REAL NOT NULL,
                y_mode TEXT NOT NULL,
                y_value REAL NOT NULL,
                z_mode TEXT NOT NULL,
                z_value REAL NOT NULL,
                UNIQUE(snapshot_id, slot)
            );",
            "CREATE TABLE IF NOT EXISTS snapshot_scalar_fields (
                snapshot_id INTEGER NOT NULL REFERENCES feature_snapshots(id) ON DELETE CASCADE,
                field_key TEXT NOT NULL,
                field_value REAL NOT NULL,
                PRIMARY KEY (snapshot_id, field_key)
            );",
            "CREATE TABLE IF NOT EXISTS history_actions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                seq INTEGER NOT NULL UNIQUE
            );",
            "CREATE TABLE IF NOT EXISTS history_action_deltas (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                action_id INTEGER NOT NULL REFERENCES history_actions(id) ON DELETE CASCADE,
                delta_index INTEGER NOT NULL,
                feature_id INTEGER NOT NULL REFERENCES features(id),
                before_snapshot_id INTEGER REFERENCES feature_snapshots(id),
                after_snapshot_id INTEGER REFERENCES feature_snapshots(id),
                UNIQUE(action_id, delta_index)
            );",
        ]),
    ]
}

fn run_migrations(conn: &Connection, from_version: u64, update_metadata: bool) -> rusqlite::Result<()> {
    for (version, statements) in migrations() {
        if version <= from_version { continue; }
        if version > SCHEMA_VERSION { break; }
        for stmt in statements {
            conn.execute_batch(stmt)?;
        }
    }
    if update_metadata && from_version < SCHEMA_VERSION {
        conn.execute(
            "UPDATE metadata SET value = ?1 WHERE key = 'schema_version'",
            params![SCHEMA_VERSION.to_string()],
        )?;
    }
    Ok(())
}

fn axis_mode(a: &AxisRef) -> &'static str {
    match a {
        AxisRef::Absolute(_) => "abs",
        AxisRef::Relative(_) => "rel",
    }
}

fn axis_from_mode(mode: &str, value: f32) -> AxisRef {
    match mode {
        "rel" => AxisRef::Relative(value),
        _ => AxisRef::Absolute(value),
    }
}

fn save_snapshot_point_ref(tx: &Transaction, snapshot_id: i64, slot: &str, pr: &PointRef) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO snapshot_point_refs (snapshot_id, slot, reference_feature_id, point_key, x_mode, x_value, y_mode, y_value, z_mode, z_value)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            snapshot_id,
            slot,
            pr.reference.map(|id| id._id() as i64),
            pr.point_key,
            axis_mode(&pr.x),
            pr.x.value() as f64,
            axis_mode(&pr.y),
            pr.y.value() as f64,
            axis_mode(&pr.z),
            pr.z.value() as f64,
        ],
    )?;
    Ok(())
}

fn save_point_ref(tx: &Transaction, owner_id: u64, slot: &str, pr: &PointRef) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO point_refs (owner_feature_id, slot, reference_feature_id, point_key, x_mode, x_value, y_mode, y_value, z_mode, z_value)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            owner_id as i64,
            slot,
            pr.reference.map(|id| id._id() as i64),
            pr.point_key,
            axis_mode(&pr.x),
            pr.x.value() as f64,
            axis_mode(&pr.y),
            pr.y.value() as f64,
            axis_mode(&pr.z),
            pr.z.value() as f64,
        ],
    )?;
    Ok(())
}

fn load_point_ref(
    conn: &Connection,
    owner_id: u64,
    slot: &str,
) -> rusqlite::Result<PointRef> {
    conn.query_row(
        "SELECT reference_feature_id, point_key, x_mode, x_value, y_mode, y_value, z_mode, z_value
         FROM point_refs WHERE owner_feature_id = ?1 AND slot = ?2",
        params![owner_id as i64, slot],
        |row| {
            let ref_id: Option<i64> = row.get(0)?;
            let point_key: String = row.get(1)?;
            let x_mode: String = row.get(2)?;
            let x_val: f64 = row.get(3)?;
            let y_mode: String = row.get(4)?;
            let y_val: f64 = row.get(5)?;
            let z_mode: String = row.get(6)?;
            let z_val: f64 = row.get(7)?;

            Ok(PointRef {
                reference: ref_id.map(|id| FeatureId::from_raw(id as u64)),
                point_key,
                x: axis_from_mode(&x_mode, x_val as f32),
                y: axis_from_mode(&y_mode, y_val as f32),
                z: axis_from_mode(&z_mode, z_val as f32),
                resolved_reference: None,
            })
        },
    )
}

fn load_snapshot_point_ref(
    conn: &Connection,
    snapshot_id: i64,
    slot: &str,
) -> rusqlite::Result<PointRef> {
    conn.query_row(
        "SELECT reference_feature_id, point_key, x_mode, x_value, y_mode, y_value, z_mode, z_value
         FROM snapshot_point_refs WHERE snapshot_id = ?1 AND slot = ?2",
        params![snapshot_id, slot],
        |row| {
            let ref_id: Option<i64> = row.get(0)?;
            let point_key: String = row.get(1)?;
            let x_mode: String = row.get(2)?;
            let x_val: f64 = row.get(3)?;
            let y_mode: String = row.get(4)?;
            let y_val: f64 = row.get(5)?;
            let z_mode: String = row.get(6)?;
            let z_val: f64 = row.get(7)?;

            Ok(PointRef {
                reference: ref_id.map(|id| FeatureId::from_raw(id as u64)),
                point_key,
                x: axis_from_mode(&x_mode, x_val as f32),
                y: axis_from_mode(&y_mode, y_val as f32),
                z: axis_from_mode(&z_mode, z_val as f32),
                resolved_reference: None,
            })
        },
    )
}

fn load_snapshot_parents(conn: &Connection, snapshot_id: i64) -> rusqlite::Result<Vec<FeatureId>> {
    let mut stmt = conn.prepare("SELECT parent_id FROM snapshot_parents WHERE snapshot_id = ?1")?;
    let rows = stmt.query_map(params![snapshot_id], |row| {
        Ok(FeatureId::from_raw(row.get::<_, i64>(0)? as u64))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn snapshot_data_kind(data: &FeatureData) -> &'static str {
    match data {
        FeatureData::GlobalPoint { .. } => "global_point",
        FeatureData::PointLight { .. } => "point_light",
        FeatureData::Room { .. } => "room",
        FeatureData::SpawnPoint { .. } => "spawn_point",
        FeatureData::Prop { .. } => "prop",
        FeatureData::AnimationDisplay { .. } => "animation_display",
        FeatureData::AnimationGrid { .. } => "animation_grid",
        FeatureData::Cuboid { .. } => "cuboid",
    }
}

fn save_feature_snapshot(
    tx: &Transaction,
    subject: FeatureId,
    snap: &FeatureSnapshot,
) -> rusqlite::Result<i64> {
    let kind = snapshot_data_kind(&snap.data);
    tx.execute(
        "INSERT INTO feature_snapshots (subject_feature_id, order_index, data_kind) VALUES (?1, ?2, ?3)",
        params![subject._id() as i64, snap.order_index as i64, kind],
    )?;
    let sid = tx.last_insert_rowid();
    for p in &snap.parents {
        tx.execute(
            "INSERT INTO snapshot_parents (snapshot_id, parent_id) VALUES (?1, ?2)",
            params![sid, p._id() as i64],
        )?;
    }
    match &snap.data {
        FeatureData::GlobalPoint { location } => {
            save_snapshot_point_ref(tx, sid, "location", location)?;
        }
        FeatureData::SpawnPoint { location, yaw } => {
            save_snapshot_point_ref(tx, sid, "location", location)?;
            tx.execute(
                "INSERT INTO snapshot_scalar_fields (snapshot_id, field_key, field_value) VALUES (?1, ?2, ?3)",
                params![sid, "yaw", *yaw as f64],
            )?;
        }
        FeatureData::Prop { location, rotation } => {
            save_snapshot_point_ref(tx, sid, "location", location)?;
            for (k, v) in [
                ("pitch", rotation.x),
                ("yaw", rotation.y),
                ("roll", rotation.z),
            ] {
                tx.execute(
                    "INSERT INTO snapshot_scalar_fields (snapshot_id, field_key, field_value) VALUES (?1, ?2, ?3)",
                    params![sid, k, v as f64],
                )?;
            }
        }
        FeatureData::AnimationDisplay { location, yaw, state } => {
            save_snapshot_point_ref(tx, sid, "location", location)?;
            // The state goes out as its index, which is stable across builds
            // by contract — see `AnimationState`.
            for (k, v) in [("yaw", *yaw), ("state", state.index() as f32)] {
                tx.execute(
                    "INSERT INTO snapshot_scalar_fields (snapshot_id, field_key, field_value) VALUES (?1, ?2, ?3)",
                    params![sid, k, v as f64],
                )?;
            }
        }
        FeatureData::AnimationGrid { location, yaw } => {
            save_snapshot_point_ref(tx, sid, "location", location)?;
            tx.execute(
                "INSERT INTO snapshot_scalar_fields (snapshot_id, field_key, field_value) VALUES (?1, ?2, ?3)",
                params![sid, "yaw", *yaw as f64],
            )?;
        }
        FeatureData::PointLight {
            location,
            intensity,
            radius,
            range,
        } => {
            save_snapshot_point_ref(tx, sid, "location", location)?;
            for (k, v) in [
                ("intensity", *intensity),
                ("radius", *radius),
                ("range_val", *range),
            ] {
                tx.execute(
                    "INSERT INTO snapshot_scalar_fields (snapshot_id, field_key, field_value) VALUES (?1, ?2, ?3)",
                    params![sid, k, v as f64],
                )?;
            }
        }
        FeatureData::Room { min, max } => {
            save_snapshot_point_ref(tx, sid, "min", min)?;
            save_snapshot_point_ref(tx, sid, "max", max)?;
        }
        FeatureData::Cuboid { min, max } => {
            let pairs = [
                ("cuboid_min_x", min.x),
                ("cuboid_min_y", min.y),
                ("cuboid_min_z", min.z),
                ("cuboid_max_x", max.x),
                ("cuboid_max_y", max.y),
                ("cuboid_max_z", max.z),
            ];
            for (k, v) in pairs {
                tx.execute(
                    "INSERT INTO snapshot_scalar_fields (snapshot_id, field_key, field_value) VALUES (?1, ?2, ?3)",
                    params![sid, k, v as f64],
                )?;
            }
        }
    }
    Ok(sid)
}

fn load_snapshot_scalar(conn: &Connection, snapshot_id: i64, key: &str) -> rusqlite::Result<f32> {
    let v: f64 = conn.query_row(
        "SELECT field_value FROM snapshot_scalar_fields WHERE snapshot_id = ?1 AND field_key = ?2",
        params![snapshot_id, key],
        |row| row.get(0),
    )?;
    Ok(v as f32)
}

/// A snapshot scalar that may simply not be there.
///
/// History is persisted, so a blueprint written before a field existed still
/// has snapshots without it — every spawn point saved before facing was a
/// thing. Those rows are not corrupt and refusing to load them would throw
/// away the file's whole undo stack over a value whose absence means "zero".
fn load_snapshot_scalar_or(conn: &Connection, snapshot_id: i64, key: &str, default: f32) -> rusqlite::Result<f32> {
    match load_snapshot_scalar(conn, snapshot_id, key) {
        Ok(v) => Ok(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(default),
        Err(e) => Err(e),
    }
}

fn load_feature_snapshot(conn: &Connection, snapshot_id: i64) -> rusqlite::Result<FeatureSnapshot> {
    let (order_index, data_kind): (i64, String) = conn.query_row(
        "SELECT order_index, data_kind FROM feature_snapshots WHERE id = ?1",
        params![snapshot_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let parents = load_snapshot_parents(conn, snapshot_id)?;
    let data = match data_kind.as_str() {
        "global_point" => {
            let location = load_snapshot_point_ref(conn, snapshot_id, "location")?;
            FeatureData::GlobalPoint { location }
        }
        "spawn_point" => {
            let location = load_snapshot_point_ref(conn, snapshot_id, "location")?;
            let yaw = load_snapshot_scalar_or(conn, snapshot_id, "yaw", 0.0)?;
            FeatureData::SpawnPoint { location, yaw }
        }
        "prop" => {
            let location = load_snapshot_point_ref(conn, snapshot_id, "location")?;
            let rotation = Vec3::new(
                load_snapshot_scalar(conn, snapshot_id, "pitch")?,
                load_snapshot_scalar(conn, snapshot_id, "yaw")?,
                load_snapshot_scalar(conn, snapshot_id, "roll")?,
            );
            FeatureData::Prop { location, rotation }
        }
        "animation_display" => {
            let location = load_snapshot_point_ref(conn, snapshot_id, "location")?;
            let yaw = load_snapshot_scalar_or(conn, snapshot_id, "yaw", 0.0)?;
            // Defaulting rather than failing: a display whose state this build
            // does not know still stands there, idle.
            let state = AnimationState::from_index(
                load_snapshot_scalar_or(conn, snapshot_id, "state", 0.0)?.max(0.0) as u32,
            );
            FeatureData::AnimationDisplay { location, yaw, state }
        }
        "animation_grid" => {
            let location = load_snapshot_point_ref(conn, snapshot_id, "location")?;
            let yaw = load_snapshot_scalar_or(conn, snapshot_id, "yaw", 0.0)?;
            FeatureData::AnimationGrid { location, yaw }
        }
        "point_light" => {
            let location = load_snapshot_point_ref(conn, snapshot_id, "location")?;
            let intensity = load_snapshot_scalar(conn, snapshot_id, "intensity")?;
            let radius = load_snapshot_scalar(conn, snapshot_id, "radius")?;
            let range = load_snapshot_scalar(conn, snapshot_id, "range_val")?;
            FeatureData::PointLight {
                location,
                intensity,
                radius,
                range,
            }
        }
        "room" => {
            let min = load_snapshot_point_ref(conn, snapshot_id, "min")?;
            let max = load_snapshot_point_ref(conn, snapshot_id, "max")?;
            FeatureData::Room { min, max }
        }
        "cuboid" => {
            let min = Vec3::new(
                load_snapshot_scalar(conn, snapshot_id, "cuboid_min_x")?,
                load_snapshot_scalar(conn, snapshot_id, "cuboid_min_y")?,
                load_snapshot_scalar(conn, snapshot_id, "cuboid_min_z")?,
            );
            let max = Vec3::new(
                load_snapshot_scalar(conn, snapshot_id, "cuboid_max_x")?,
                load_snapshot_scalar(conn, snapshot_id, "cuboid_max_y")?,
                load_snapshot_scalar(conn, snapshot_id, "cuboid_max_z")?,
            );
            FeatureData::Cuboid { min, max }
        }
        other => {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "Unknown snapshot data_kind: {}",
                other
            )));
        }
    };
    Ok(FeatureSnapshot {
        data,
        parents,
        order_index: order_index as usize,
    })
}

fn load_applied_actions(conn: &Connection) -> rusqlite::Result<Vec<Action>> {
    let mut action_ids: Vec<i64> = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT id FROM history_actions ORDER BY seq ASC")?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        for r in rows {
            action_ids.push(r?);
        }
    }
    let mut actions = Vec::new();
    for aid in action_ids {
        let mut stmt = conn.prepare(
            "SELECT delta_index, feature_id, before_snapshot_id, after_snapshot_id
             FROM history_action_deltas WHERE action_id = ?1 ORDER BY delta_index ASC",
        )?;
        let rows = stmt.query_map(params![aid], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })?;
        let mut deltas: Vec<FeatureDelta> = Vec::new();
        for r in rows {
            let (_didx, fid, before_id, after_id) = r?;
            let before = match before_id {
                Some(id) => Some(load_feature_snapshot(conn, id)?),
                None => None,
            };
            let after = match after_id {
                Some(id) => Some(load_feature_snapshot(conn, id)?),
                None => None,
            };
            deltas.push(FeatureDelta {
                feature_id: FeatureId::from_raw(fid as u64),
                before,
                after,
            });
        }
        actions.push(Action { deltas });
    }
    Ok(actions)
}

pub fn save(path: &Path, features: &FeatureTimeline, metadata: &MapMetadata) -> rusqlite::Result<()> {
    let backup_path = path.with_extension(format!("{}.{}", MAP_BLUEPRINT_EXTENSION, MAP_BACKUP_EXTENSION));
    let had_existing = path.exists();

    if had_existing {
        std::fs::copy(path, &backup_path).map_err(|e| {
            rusqlite::Error::InvalidParameterName(format!("Failed to create backup: {}", e))
        })?;
    }

    let result = save_inner(path, features, metadata);

    if result.is_err() && had_existing {
        info!("Save failed, restoring from backup");
        let _ = std::fs::copy(&backup_path, path);
    }

    if had_existing && result.is_ok() {
        let _ = std::fs::remove_file(&backup_path);
    }

    result
}

fn save_inner(path: &Path, features: &FeatureTimeline, metadata: &MapMetadata) -> rusqlite::Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch("DROP TABLE IF EXISTS history_action_deltas;
                        DROP TABLE IF EXISTS history_actions;
                        DROP TABLE IF EXISTS snapshot_scalar_fields;
                        DROP TABLE IF EXISTS snapshot_point_refs;
                        DROP TABLE IF EXISTS snapshot_parents;
                        DROP TABLE IF EXISTS feature_snapshots;
                        DROP TABLE IF EXISTS scalar_fields;
                        DROP TABLE IF EXISTS point_refs;
                        DROP TABLE IF EXISTS feature_parents;
                        DROP TABLE IF EXISTS features;
                        DROP TABLE IF EXISTS editor_meta;
                        DROP TABLE IF EXISTS metadata;")?;
    run_migrations(&conn, 0, false)?;

    let tx = conn.unchecked_transaction()?;

    tx.execute(
        "INSERT INTO metadata (key, value) VALUES ('schema_version', ?1)",
        params![SCHEMA_VERSION.to_string()],
    )?;

    metadata.insert_rows(&tx)?;

    tx.execute(
        "INSERT INTO editor_meta (id_counter, rollback_bar) VALUES (?1, ?2)",
        params![features.id_counter() as i64, features.rollback_bar() as i64],
    )?;

    for (idx, id) in features.feature_order().iter().enumerate() {
        let Some(feature) = features.features_map().get(id) else { continue };
        let raw_id = id._id() as i64;

        tx.execute(
            "INSERT INTO features (id, type_key, order_index) VALUES (?1, ?2, ?3)",
            params![raw_id, feature.object().type_key(), idx as i64],
        )?;

        for parent_id in feature.parents() {
            tx.execute(
                "INSERT INTO feature_parents (feature_id, parent_id) VALUES (?1, ?2)",
                params![raw_id, parent_id._id() as i64],
            )?;
        }

        let obj = feature.object();
        for slot in obj.point_ref_slots() {
            if let Some(pr) = obj.get_point_ref(slot) {
                save_point_ref(&tx, id._id(), slot, pr)?;
            }
        }

        for (key, value) in obj.scalar_fields() {
            tx.execute(
                "INSERT INTO scalar_fields (owner_feature_id, field_key, field_value) VALUES (?1, ?2, ?3)",
                params![raw_id, key, value as f64],
            )?;
        }
    }

    for (seq, action) in features.applied_actions().iter().enumerate() {
        tx.execute(
            "INSERT INTO history_actions (seq) VALUES (?1)",
            params![seq as i64],
        )?;
        let action_id = tx.last_insert_rowid();
        for (di, delta) in action.deltas.iter().enumerate() {
            let before_id: Option<i64> = match &delta.before {
                Some(s) => Some(save_feature_snapshot(&tx, delta.feature_id, s)?),
                None => None,
            };
            let after_id: Option<i64> = match &delta.after {
                Some(s) => Some(save_feature_snapshot(&tx, delta.feature_id, s)?),
                None => None,
            };
            tx.execute(
                "INSERT INTO history_action_deltas (action_id, delta_index, feature_id, before_snapshot_id, after_snapshot_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    action_id,
                    di as i64,
                    delta.feature_id._id() as i64,
                    before_id,
                    after_id,
                ],
            )?;
        }
    }

    tx.commit()?;
    Ok(())
}

pub fn load(path: &Path) -> rusqlite::Result<LoadedBlueprint> {
    let conn = Connection::open(path)?;

    let file_version: u64 = conn.query_row(
        "SELECT value FROM metadata WHERE key = 'schema_version'",
        [],
        |row| {
            let v: String = row.get(0)?;
            v.parse::<u64>().map_err(|_| {
                rusqlite::Error::InvalidParameterName("schema_version is not a valid u64".into())
            })
        },
    )?;
    if file_version > SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidParameterName(
            format!(
                "File schema version {} is newer than supported version {}",
                file_version, SCHEMA_VERSION
            ),
        ));
    }

    if file_version < SCHEMA_VERSION {
        run_migrations(&conn, file_version, true)?;
    }

    let (id_counter, rollback_bar) = conn.query_row(
        "SELECT id_counter, rollback_bar FROM editor_meta LIMIT 1",
        [],
        |row| {
            let ic: i64 = row.get(0)?;
            let rb: i64 = row.get(1)?;
            Ok((ic as u64, rb as u64))
        },
    )?;

    let mut feature_rows: Vec<(u64, String, usize)> = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT id, type_key, order_index FROM features ORDER BY order_index")?;
        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let type_key: String = row.get(1)?;
            let order_index: i64 = row.get(2)?;
            Ok((id as u64, type_key, order_index as usize))
        })?;
        for row in rows {
            feature_rows.push(row?);
        }
    }

    let mut parent_map: HashMap<u64, Vec<FeatureId>> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT feature_id, parent_id FROM feature_parents")?;
        let rows = stmt.query_map([], |row| {
            let feature_id: i64 = row.get(0)?;
            let parent_id: i64 = row.get(1)?;
            Ok((feature_id as u64, parent_id as u64))
        })?;
        for row in rows {
            let (aid, pid) = row?;
            parent_map.entry(aid).or_default().push(FeatureId::from_raw(pid));
        }
    }

    let mut scalar_map: HashMap<u64, Vec<(String, f32)>> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT owner_feature_id, field_key, field_value FROM scalar_fields")?;
        let rows = stmt.query_map([], |row| {
            let owner: i64 = row.get(0)?;
            let key: String = row.get(1)?;
            let val: f64 = row.get(2)?;
            Ok((owner as u64, key, val as f32))
        })?;
        for row in rows {
            let (owner, key, val) = row?;
            scalar_map.entry(owner).or_default().push((key, val));
        }
    }

    let mut features_map: HashMap<FeatureId, Feature> = HashMap::new();
    let mut feature_order: Vec<FeatureId> = Vec::new();

    for (raw_id, type_key, _order_index) in &feature_rows {
        let id = FeatureId::from_raw(*raw_id);
        feature_order.push(id);

        let Some(mut obj) = create_object_from_type_key(type_key) else {
            continue;
        };

        let slots: Vec<String> = obj.point_ref_slots().iter().map(|s| s.to_string()).collect();
        for slot in &slots {
            let pr = load_point_ref(&conn, *raw_id, slot)?;
            if let Some(target) = obj.get_point_ref_mut(slot) {
                *target = pr;
            }
        }

        if let Some(scalars) = scalar_map.get(raw_id) {
            for (key, val) in scalars {
                obj.set_scalar_field(key, *val);
            }
        }

        let parents = parent_map.remove(raw_id).unwrap_or_default();
        let feature = Feature::new(id, obj, parents);
        features_map.insert(id, feature);
    }

    let actions = load_applied_actions(&conn)?;
    let mut editor_features =
        FeatureTimeline::from_parts(features_map, feature_order, id_counter, rollback_bar, actions);

    for id in editor_features.feature_order().to_vec() {
        if let Some(mut feature) = editor_features.features_mut().remove(&id) {
            feature.object_mut().resolve_references(editor_features.features_map());
            editor_features.features_mut().insert(id, feature);
        }
    }

    let map_metadata = MapMetadata::load_from_connection(&conn)?;

    Ok(LoadedBlueprint {
        timeline: editor_features,
        metadata: map_metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::global_point::GlobalPoint;
    use crate::editor::spawn_point::SpawnPoint;
    use crate::tool::room::Room;

    fn count_spawns(timeline: &FeatureTimeline) -> usize {
        timeline
            .active_features()
            .filter(|(_, f)| f.object().type_key() == "spawn_point")
            .count()
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("grackle-save-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.{MAP_BLUEPRINT_EXTENSION}"));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// Registering a feature type takes edits in three places that do not
    /// reference each other — `create_object_from_type_key`, the `FeatureData`
    /// variant, and `blank_object` — plus two more here. Miss one and load
    /// half-works: the file saves, and the feature is simply not there when it
    /// comes back. Nothing in the type system says otherwise, so a round trip
    /// has to.
    #[test]
    fn a_spawn_point_survives_a_save_and_load() {
        let mut timeline = FeatureTimeline::default();
        timeline.apply_feature(Box::new(SpawnPoint::new(3.0, 0.0, -4.0)));
        timeline.apply_feature(Box::new(GlobalPoint::new(1.0, 2.0, 3.0)));

        let path = temp_path("spawn-round-trip");
        save(&path, &timeline, &MapMetadata::default()).unwrap();
        let loaded = load(&path).unwrap();

        let spawns: Vec<Vec3> = loaded
            .timeline
            .active_features()
            .filter(|(_, f)| f.object().type_key() == "spawn_point")
            .map(|(_, f)| f.object().get_point("").unwrap())
            .collect();

        assert_eq!(spawns, vec![Vec3::new(3.0, 0.0, -4.0)]);

        // And the feature it was saved alongside is still there, so this is
        // not passing because everything came back empty.
        assert_eq!(
            loaded
                .timeline
                .active_features()
                .filter(|(_, f)| f.object().type_key() == "global_point")
                .count(),
            1
        );
    }

    /// Undo history is persisted too, and snapshots take their own pair of
    /// registrations in `snapshot_data_kind` and `load_feature_snapshot`.
    #[test]
    fn a_spawn_point_in_the_undo_history_survives_a_save_and_load() {
        let mut timeline = FeatureTimeline::default();
        let id = timeline.apply_feature(Box::new(SpawnPoint::new(5.0, 1.0, 2.0)));

        let path = temp_path("spawn-history-round-trip");
        save(&path, &timeline, &MapMetadata::default()).unwrap();
        let mut loaded = load(&path).unwrap();

        // Rolling back and forward again reconstructs the feature from its
        // snapshot rather than from the features table.
        loaded.timeline.undo();
        assert_eq!(count_spawns(&loaded.timeline), 0, "undo left the spawn point active");

        loaded.timeline.redo();
        let spawns: Vec<Vec3> = loaded
            .timeline
            .active_features()
            .filter(|(_, f)| f.object().type_key() == "spawn_point")
            .map(|(_, f)| f.object().get_point("").unwrap())
            .collect();
        assert_eq!(spawns, vec![Vec3::new(5.0, 1.0, 2.0)]);
    }

    /// A prop's whole point is that it is turned, so a round trip that only
    /// carried its position back would look like a pass and lose every
    /// facing on the map.
    #[test]
    fn a_prop_survives_a_save_and_load_with_its_rotation() {
        use crate::editor::prop::Prop;

        let rotation = Vec3::new(0.25, -1.5, 0.75);
        let mut timeline = FeatureTimeline::default();
        timeline.apply_feature(Box::new(
            Prop::new(2.0, 3.0, -1.0).with_rotation(rotation),
        ));

        let path = temp_path("prop-round-trip");
        save(&path, &timeline, &MapMetadata::default()).unwrap();
        let mut loaded = load(&path).unwrap();

        let read_back = |timeline: &FeatureTimeline| -> Vec<(Vec3, Vec3)> {
            timeline
                .active_features()
                .filter(|(_, f)| f.object().type_key() == "prop")
                .map(|(_, f)| (f.object().get_point("").unwrap(), f.object().euler_angles()))
                .collect()
        };

        assert_eq!(
            read_back(&loaded.timeline),
            vec![(Vec3::new(2.0, 3.0, -1.0), rotation)]
        );

        // And again out of the persisted history, which is registered
        // separately from the features table and fails separately.
        loaded.timeline.undo();
        assert!(read_back(&loaded.timeline).is_empty(), "undo left the prop active");
        loaded.timeline.redo();
        assert_eq!(
            read_back(&loaded.timeline),
            vec![(Vec3::new(2.0, 3.0, -1.0), rotation)]
        );
    }

    /// Registering a feature type takes edits in seven places that do not
    /// reference each other, and a missed one loses the feature quietly: the
    /// file saves, and the display is simply absent when it comes back. The
    /// chosen state is the part most likely to survive halfway, since it goes
    /// to disk as a number and a wrong one still loads as a valid state.
    #[test]
    fn an_animation_display_survives_a_save_and_load_with_its_state() {
        use crate::common::skeleton::AnimationState;
        use crate::editor::animation_display::AnimationDisplay;

        let mut timeline = FeatureTimeline::default();
        timeline.apply_feature(Box::new(
            AnimationDisplay::new(4.0, 0.0, -2.0)
                .with_state(AnimationState::PushingWall)
                .with_yaw(1.25),
        ));

        let path = temp_path("animation-display-round-trip");
        save(&path, &timeline, &MapMetadata::default()).unwrap();
        let mut loaded = load(&path).unwrap();

        let read_back = |timeline: &FeatureTimeline| -> Vec<(Vec3, f32, f32)> {
            timeline
                .active_features()
                .filter(|(_, f)| f.object().type_key() == "animation_display")
                .map(|(_, f)| {
                    let scalars: HashMap<&str, f32> = f.object().scalar_fields().into_iter().collect();
                    (
                        f.object().get_point("").unwrap(),
                        scalars["yaw"],
                        scalars["state"],
                    )
                })
                .collect()
        };

        let expected = vec![(
            Vec3::new(4.0, 0.0, -2.0),
            1.25,
            AnimationState::PushingWall.index() as f32,
        )];
        assert_eq!(read_back(&loaded.timeline), expected);

        // And again out of the persisted history, which is registered
        // separately from the features table and fails separately.
        loaded.timeline.undo();
        assert!(read_back(&loaded.timeline).is_empty(), "undo left the display active");
        loaded.timeline.redo();
        assert_eq!(read_back(&loaded.timeline), expected);
    }

    /// The grid carries almost nothing of its own — a point and a facing — so
    /// the thing that breaks is registration rather than data: miss one of the
    /// seven places and the map saves fine and comes back sixty bodies short.
    #[test]
    fn an_animation_grid_survives_a_save_and_load() {
        use crate::editor::animation_grid::AnimationGrid;

        let mut timeline = FeatureTimeline::default();
        timeline.apply_feature(Box::new(AnimationGrid::new(-3.0, 0.5, 7.0).with_yaw(-0.75)));

        let path = temp_path("animation-grid-round-trip");
        save(&path, &timeline, &MapMetadata::default()).unwrap();
        let mut loaded = load(&path).unwrap();

        let read_back = |timeline: &FeatureTimeline| -> Vec<(Vec3, Vec3)> {
            timeline
                .active_features()
                .filter(|(_, f)| f.object().type_key() == "animation_grid")
                .map(|(_, f)| (f.object().get_point("").unwrap(), f.object().euler_angles()))
                .collect()
        };

        let expected = vec![(Vec3::new(-3.0, 0.5, 7.0), Vec3::new(0.0, -0.75, 0.0))];
        assert_eq!(read_back(&loaded.timeline), expected);

        loaded.timeline.undo();
        assert!(read_back(&loaded.timeline).is_empty(), "undo left the grid active");
        loaded.timeline.redo();
        assert_eq!(read_back(&loaded.timeline), expected);
    }

    /// Facing rides along on the spawn point as a scalar, which is a separate
    /// registration from the point itself in both the live tables and the
    /// snapshots. A spawn that comes back pointing at zero is a spawn that
    /// drops everyone facing a wall.
    #[test]
    fn a_spawn_point_keeps_its_facing_across_a_save_and_load() {
        let yaw = 1.25;
        let mut timeline = FeatureTimeline::default();
        timeline.apply_feature(Box::new(SpawnPoint::new(0.0, 0.0, 0.0).with_yaw(yaw)));

        let path = temp_path("spawn-yaw-round-trip");
        save(&path, &timeline, &MapMetadata::default()).unwrap();
        let mut loaded = load(&path).unwrap();

        let read_back = |timeline: &FeatureTimeline| -> Vec<f32> {
            timeline
                .active_features()
                .filter(|(_, f)| f.object().type_key() == "spawn_point")
                .map(|(_, f)| f.object().euler_angles().y)
                .collect()
        };

        assert_eq!(read_back(&loaded.timeline), vec![yaw]);

        loaded.timeline.undo();
        loaded.timeline.redo();
        assert_eq!(read_back(&loaded.timeline), vec![yaw], "the history lost the facing");
    }

    /// What a new map has to give you, and no more than that.
    ///
    /// The template is a checked-in binary, so nothing about it shows up in a
    /// diff — but most of what is in it is taste and will change. Pinning the
    /// spawn point's coordinates would just be a test that has to be edited
    /// every time someone moves it. These are the things whose absence would
    /// actually break something: no room and there is nothing to stand in, no
    /// light and you cannot see it, no spawn with headroom and pressing F5
    /// falls back to guessing.
    #[test]
    fn a_new_map_can_be_stood_in_lit_and_spawned_into() {
        use crate::common::class::{body_centre_from_feet, CLASS_HALF_EXTENTS};
        use crate::game::collision::CollisionWorld;

        let path = PathBuf::from(format!(
            "assets/default/blueprints/new.{}",
            MAP_BLUEPRINT_EXTENSION
        ));
        let loaded = load(&path).expect("the shipped template must load");

        let rooms: Vec<Room> = loaded
            .timeline
            .active_features()
            .filter_map(|(_, f)| f.object().drag_handle_bounds())
            .map(|(min, max)| Room::new(min, max))
            .collect();
        assert!(!rooms.is_empty(), "the template has no room to stand in");

        let lights = loaded
            .timeline
            .active_features()
            .filter(|(_, f)| f.object().type_key() == "grackle_point_light")
            .count();
        assert!(lights > 0, "the template has no light to see by");

        let spawns: Vec<Vec3> = loaded
            .timeline
            .active_features()
            .filter(|(_, f)| f.object().type_key() == "spawn_point")
            .map(|(_, f)| f.object().get_point("").unwrap())
            .collect();
        assert!(!spawns.is_empty(), "the template has no spawn point");

        let mut world = CollisionWorld::default();
        world.rebuild(&rooms);
        assert!(
            spawns
                .iter()
                .any(|feet| world.fits(body_centre_from_feet(*feet), CLASS_HALF_EXTENTS)),
            "no spawn point in the template has room to stand: {spawns:?}"
        );
    }

    /// Rooms are the other thing the game reads, and they go through the same
    /// registration. Cheap to assert while the harness is here.
    #[test]
    fn a_room_survives_a_save_and_load() {
        use crate::editor::editor_room::EditorRoom;

        let mut timeline = FeatureTimeline::default();
        timeline.apply_feature(Box::new(EditorRoom::from_point_refs(
            PointRef::absolute(-2.0, 0.0, -2.0),
            PointRef::absolute(2.0, 3.0, 2.0),
        )));

        let path = temp_path("room-round-trip");
        save(&path, &timeline, &MapMetadata::default()).unwrap();
        let loaded = load(&path).unwrap();

        let bounds: Vec<(Vec3, Vec3)> = loaded
            .timeline
            .active_features()
            .filter_map(|(_, f)| f.object().drag_handle_bounds())
            .collect();
        assert_eq!(bounds, vec![(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 3.0, 2.0))]);
        let _ = Room::default();
    }
}
