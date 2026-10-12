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
    "api_keys", "sessions", "bench_runs", "mem_calibration", "downloads", "gh_sources", "ppl_runs", "mcp_servers",
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
             CREATE INDEX IF NOT EXISTS request_log_ts ON request_log(ts);
             CREATE TABLE IF NOT EXISTS usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL,
                preset_id TEXT NOT NULL DEFAULT '', preset TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '', alias TEXT NOT NULL DEFAULT '',
                user_id TEXT NOT NULL DEFAULT '', user TEXT NOT NULL DEFAULT '',
                ip TEXT NOT NULL DEFAULT '', path TEXT NOT NULL DEFAULT '', status INTEGER NOT NULL DEFAULT 0,
                prompt INTEGER NOT NULL DEFAULT 0, cached INTEGER NOT NULL DEFAULT 0, completion INTEGER NOT NULL DEFAULT 0,
                ms INTEGER NOT NULL DEFAULT 0, tps REAL);
             CREATE INDEX IF NOT EXISTS usage_ts ON usage(ts);",
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

    // ---------------- token usage analytics
    pub fn usage_insert(&self, u: &UsageRow, retention_days: u32) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO usage(ts,preset_id,preset,model,alias,user_id,user,ip,path,status,prompt,cached,completion,ms,tps)
             VALUES(strftime('%s','now'),?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![u.preset_id, u.preset, u.model, u.alias, u.user_id, u.user, u.ip, u.path, u.status, u.prompt, u.cached, u.completion, u.ms, u.tps],
        )?;
        if retention_days > 0 && conn.last_insert_rowid() % 200 == 0 {
            conn.execute("DELETE FROM usage WHERE ts < strftime('%s','now') - ?1", params![retention_days as i64 * 86400])?;
        }
        Ok(())
    }

    pub fn usage_clear(&self) -> Result<usize> {
        Ok(self.conn.lock().execute("DELETE FROM usage", [])?)
    }

    /// Aggregate usage in [from, to): totals, per-group rows and a time series per group.
    /// `group`: model | preset | alias | user. `bucket`: seconds (3600/86400…), `tz`: offset seconds east of UTC.
    pub fn usage_query(&self, q: &UsageQuery) -> Result<serde_json::Value> {
        use serde_json::json;
        let (key, label) = match q.group.as_str() {
            "preset" => ("preset_id", "MAX(preset)"),
            "alias" => ("alias", "alias"),
            "user" => ("user_id", "MAX(user)"),
            _ => ("model", "model"),
        };
        let conn = self.conn.lock();
        let mut where_ = String::from("ts >= ?1 AND ts < ?2");
        let filt = q.filter.clone().unwrap_or_default();
        if !filt.is_empty() {
            where_ += &format!(" AND {key} = ?3");
        }
        let args: Vec<rusqlite::types::Value> = {
            let mut a: Vec<rusqlite::types::Value> = vec![q.from.into(), q.to.into()];
            if !filt.is_empty() {
                a.push(filt.clone().into());
            }
            a
        };
        let agg = "COUNT(*), COALESCE(SUM(prompt),0), COALESCE(SUM(cached),0), COALESCE(SUM(completion),0), COALESCE(SUM(status >= 400),0), COALESCE(SUM(ms),0), AVG(tps)";
        let row_json = |r: &rusqlite::Row, o: usize| -> rusqlite::Result<serde_json::Value> {
            Ok(json!({
                "requests": r.get::<_, i64>(o)?, "prompt": r.get::<_, i64>(o + 1)?, "cached": r.get::<_, i64>(o + 2)?,
                "completion": r.get::<_, i64>(o + 3)?, "errors": r.get::<_, i64>(o + 4)?, "ms": r.get::<_, i64>(o + 5)?,
                "avg_tps": r.get::<_, Option<f64>>(o + 6)?,
            }))
        };
        let totals = conn.query_row(&format!("SELECT {agg} FROM usage WHERE {where_}"), rusqlite::params_from_iter(args.iter()), |r| row_json(r, 0))?;
        let mut groups = vec![];
        {
            let mut st = conn.prepare(&format!("SELECT {key}, {label}, {agg} FROM usage WHERE {where_} GROUP BY {key} ORDER BY SUM(prompt + completion) DESC, COUNT(*) DESC"))?;
            let rows = st.query_map(rusqlite::params_from_iter(args.iter()), |r| {
                let mut v = row_json(r, 2)?;
                v["key"] = json!(r.get::<_, String>(0)?);
                v["label"] = json!(r.get::<_, String>(1)?);
                Ok(v)
            })?;
            for r in rows {
                groups.push(r?);
            }
        }
        let bucket = q.bucket.max(60);
        let mut series = vec![];
        {
            let sql = format!(
                "SELECT ((ts + ?{n}) / {bucket}) * {bucket} - ?{n} AS b, {key}, COUNT(*), SUM(prompt), SUM(cached), SUM(completion)
                 FROM usage WHERE {where_} GROUP BY b, {key} ORDER BY b",
                n = args.len() + 1
            );
            let mut a2 = args.clone();
            a2.push(q.tz.into());
            let mut st = conn.prepare(&sql)?;
            let rows = st.query_map(rusqlite::params_from_iter(a2.iter()), |r| {
                Ok(json!({"t": r.get::<_, i64>(0)?, "key": r.get::<_, String>(1)?, "requests": r.get::<_, i64>(2)?,
                    "prompt": r.get::<_, i64>(3)?, "cached": r.get::<_, i64>(4)?, "completion": r.get::<_, i64>(5)?}))
            })?;
            for r in rows {
                series.push(r?);
            }
        }
        Ok(json!({"from": q.from, "to": q.to, "group": q.group, "bucket": bucket, "totals": totals, "groups": groups, "series": series}))
    }

    /// Raw rows for CSV export.
    pub fn usage_rows(&self, from: i64, to: i64) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT ts,preset,model,alias,user,ip,path,status,prompt,cached,completion,ms,tps FROM usage WHERE ts >= ?1 AND ts < ?2 ORDER BY id")?;
        let rows = st.query_map(params![from, to], |r| {
            Ok(serde_json::json!({"ts": r.get::<_, i64>(0)?, "preset": r.get::<_, String>(1)?, "model": r.get::<_, String>(2)?,
                "alias": r.get::<_, String>(3)?, "user": r.get::<_, String>(4)?, "ip": r.get::<_, String>(5)?, "path": r.get::<_, String>(6)?,
                "status": r.get::<_, i64>(7)?, "prompt": r.get::<_, i64>(8)?, "cached": r.get::<_, i64>(9)?, "completion": r.get::<_, i64>(10)?,
                "ms": r.get::<_, i64>(11)?, "tps": r.get::<_, Option<f64>>(12)?}))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Consistent snapshot of the DB into a file (for backups).
    pub fn backup_to(&self, path: &Path) -> Result<()> {
        let conn = self.conn.lock();
        let _ = std::fs::remove_file(path);
        conn.execute("VACUUM INTO ?1", params![path.to_string_lossy()])?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct UsageRow {
    pub preset_id: String,
    pub preset: String,
    pub model: String,
    pub alias: String,
    pub user_id: String,
    pub user: String,
    pub ip: String,
    pub path: String,
    pub status: u16,
    pub prompt: i64,
    pub cached: i64,
    pub completion: i64,
    pub ms: i64,
    pub tps: Option<f64>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct UsageQuery {
    pub from: i64,
    pub to: i64,
    #[serde(default)]
    pub group: String,
    #[serde(default = "default_bucket")]
    pub bucket: i64,
    #[serde(default)]
    pub tz: i64,
    pub filter: Option<String>,
}
fn default_bucket() -> i64 {
    3600
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

    #[test]
    fn usage_aggregation() {
        let s = Store::memory().unwrap();
        let row = |model: &str, user: &str, p: i64, c: i64, o: i64| UsageRow { model: model.into(), user_id: user.into(), user: user.into(), preset_id: "p1".into(), preset: "P".into(), prompt: p, cached: c, completion: o, status: 200, ..Default::default() };
        s.usage_insert(&row("a.gguf", "k1", 100, 40, 10), 0).unwrap();
        s.usage_insert(&row("a.gguf", "k2", 50, 0, 5), 0).unwrap();
        s.usage_insert(&row("b.gguf", "k1", 10, 0, 1), 0).unwrap();
        let q = |g: &str| UsageQuery { from: 0, to: i64::MAX / 2, group: g.into(), bucket: 3600, tz: 0, filter: None };
        let r = s.usage_query(&q("model")).unwrap();
        assert_eq!(r["totals"]["prompt"], 160);
        assert_eq!(r["totals"]["cached"], 40);
        assert_eq!(r["groups"][0]["key"], "a.gguf");
        assert_eq!(r["groups"][0]["completion"], 15);
        let r = s.usage_query(&q("user")).unwrap();
        assert_eq!(r["groups"].as_array().unwrap().len(), 2);
        assert_eq!(r["groups"][0]["key"], "k1");
        assert_eq!(r["groups"][0]["requests"], 2);
        let mut f = q("user");
        f.filter = Some("k2".into());
        assert_eq!(s.usage_query(&f).unwrap()["totals"]["requests"], 1);
        assert_eq!(s.usage_rows(0, i64::MAX / 2).unwrap().len(), 3);
    }
}
