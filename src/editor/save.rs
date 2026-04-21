use std::path::{Path, PathBuf};
use bevy::platform::collections::HashMap;
use bevy::prelude::info;
use rusqlite::{Connection, Transaction, params};
use crate::constants::{SCHEMA_VERSION, MAP_BLUEPRINT_EXTENSION, MAP_BACKUP_EXTENSION};
use crate::editor::editable::{
    AxisRef, Feature, FeatureId, FeatureTimeline, PointRef,
    create_object_from_type_key,
};

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

pub fn save(path: &Path, features: &FeatureTimeline) -> rusqlite::Result<()> {
    let backup_path = path.with_extension(format!("{}.{}", MAP_BLUEPRINT_EXTENSION, MAP_BACKUP_EXTENSION));
    let had_existing = path.exists();

    if had_existing {
        std::fs::copy(path, &backup_path).map_err(|e| {
            rusqlite::Error::InvalidParameterName(format!("Failed to create backup: {}", e))
        })?;
    }

    let result = save_inner(path, features);

    if result.is_err() && had_existing {
        info!("Save failed, restoring from backup");
        let _ = std::fs::copy(&backup_path, path);
    }

    if had_existing && result.is_ok() {
        let _ = std::fs::remove_file(&backup_path);
    }

    result
}

fn save_inner(path: &Path, features: &FeatureTimeline) -> rusqlite::Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch("DROP TABLE IF EXISTS scalar_fields;
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

    tx.commit()?;
    Ok(())
}

pub fn load(path: &Path) -> rusqlite::Result<FeatureTimeline> {
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

    let mut editor_features = FeatureTimeline::from_parts(features_map, feature_order, id_counter, rollback_bar);

    for id in editor_features.feature_order().to_vec() {
        if let Some(mut feature) = editor_features.features_mut().remove(&id) {
            feature.object_mut().resolve_references(editor_features.features_map());
            editor_features.features_mut().insert(id, feature);
        }
    }

    Ok(editor_features)
}
