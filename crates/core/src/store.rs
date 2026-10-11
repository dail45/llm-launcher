//! SQLite-backed storage. Entities are stored as JSON documents in per-collection tables,
//! which keeps the schema flexible while still giving durability and transactions.
use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use std::path::Path;
use std::sync::Arc;

pub const COLLECTIONS: &[&str] = &[
    "backends", "model_dirs", "models", "presets", "preset_history", "aliases", "alias_groups",
    "api_keys", "sessions", "bench_runs", "mem_calibration", "downloads", "gh_sources",
];

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }
    pub fn memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }
    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;")?;
        for c in COLLECTIONS {
            conn.execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {c} (id TEXT PRIMARY KEY, data TEXT NOT NULL, updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')));"
            ))?;
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS request_log (id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, data TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS security_log (id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, data TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS request_log_ts ON request_log(ts);",
        )?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    fn check(c: &str) {
        assert!(COLLECTIONS.contains(&c), "unknown collection {c}");
    }

    pub fn put<T: Serialize>(&self, coll: &str, id: &str, v: &T) -> Result<()> {
        Self::check(coll);
        let data = serde_json::to_string(v)?;
        self.conn.lock().execute(
            &format!("INSERT INTO {coll}(id, data, updated_at) VALUES(?1, ?2, strftime('%s','now')) ON CONFLICT(id) DO UPDATE SET data=excluded.data, updated_at=excluded.updated_at"),
            params![id, data],
        )?;
        Ok(())
    }
    pub fn get<T: DeserializeOwned>(&self, coll: &str, id: &str) -> Result<Option<T>> {
        Self::check(coll);
        let s: Option<String> = self
            .conn
            .lock()
            .query_row(&format!("SELECT data FROM {coll} WHERE id=?1"), params![id], |r| r.get(0))
            .optional()?;
        Ok(match s {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        })
    }
    pub fn list<T: DeserializeOwned>(&self, coll: &str) -> Result<Vec<T>> {
        Self::check(coll);
        let conn = self.conn.lock();
        let mut st = conn.prepare(&format!("SELECT data FROM {coll} ORDER BY rowid"))?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = vec![];
        for r in rows {
            match serde_json::from_str(&r?) {
                Ok(v) => out.push(v),
                Err(e) => tracing::warn!("skip corrupted row in {coll}: {e}"),
            }
        }
        Ok(out)
    }
    pub fn delete(&self, coll: &str, id: &str) -> Result<bool> {
        Self::check(coll);
        Ok(self.conn.lock().execute(&format!("DELETE FROM {coll} WHERE id=?1"), params![id])? > 0)
    }

    pub fn kv_get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let s: Option<String> =
            self.conn.lock().query_row("SELECT value FROM kv WHERE key=?1", params![key], |r| r.get(0)).optional()?;
        Ok(match s {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        })
    }
    pub fn kv_set<T: Serialize>(&self, key: &str, v: &T) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, serde_json::to_string(v)?],
        )?;
        Ok(())
    }

    pub fn log_append(&self, table: &str, v: &serde_json::Value, keep: i64) -> Result<()> {
        assert!(table == "request_log" || table == "security_log");
        let conn = self.conn.lock();
        conn.execute(&format!("INSERT INTO {table}(ts,data) VALUES(strftime('%s','now'),?1)"), params![v.to_string()])?;
        conn.execute(&format!("DELETE FROM {table} WHERE id <= (SELECT MAX(id) FROM {table}) - ?1"), params![keep])?;
        Ok(())
    }
    pub fn log_list(&self, table: &str, limit: i64) -> Result<Vec<serde_json::Value>> {
        assert!(table == "request_log" || table == "security_log");
        let conn = self.conn.lock();
        let mut st = conn.prepare(&format!("SELECT ts, data FROM {table} ORDER BY id DESC LIMIT ?1"))?;
        let rows = st.query_map(params![limit], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = vec![];
        for r in rows {
            let (ts, d) = r?;
            let mut v: serde_json::Value = serde_json::from_str(&d).unwrap_or_default();
            if let Some(o) = v.as_object_mut() {
                o.insert("ts".into(), ts.into());
            }
            out.push(v);
        }
        Ok(out)
    }

    /// Consistent snapshot of the DB into a file (for backups).
    pub fn backup_to(&self, path: &Path) -> Result<()> {
        let conn = self.conn.lock();
        let _ = std::fs::remove_file(path);
        conn.execute("VACUUM INTO ?1", params![path.to_string_lossy()])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let s = Store::memory().unwrap();
        s.put("models", "a", &serde_json::json!({"x": 1})).unwrap();
        let v: Option<serde_json::Value> = s.get("models", "a").unwrap();
        assert_eq!(v.unwrap()["x"], 1);
        assert_eq!(s.list::<serde_json::Value>("models").unwrap().len(), 1);
        assert!(s.delete("models", "a").unwrap());
        s.kv_set("k", &5).unwrap();
        assert_eq!(s.kv_get::<i32>("k").unwrap(), Some(5));
        s.log_append("request_log", &serde_json::json!({"a":1}), 2).unwrap();
        s.log_append("request_log", &serde_json::json!({"a":2}), 2).unwrap();
        s.log_append("request_log", &serde_json::json!({"a":3}), 2).unwrap();
        assert_eq!(s.log_list("request_log", 10).unwrap().len(), 2);
    }
}
