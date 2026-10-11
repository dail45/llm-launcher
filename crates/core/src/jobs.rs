//! Background jobs (backend installs, benchmarks, scans) with progress events.
use crate::events::{Bus, Event};
use crate::types::{new_id, now};
use parking_lot::RwLock;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
pub struct JobInfo {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub progress: Option<f64>,
    pub message: String,
    pub result: Value,
    pub created_at: i64,
}

#[derive(Default)]
pub struct Jobs {
    map: RwLock<HashMap<String, JobInfo>>,
}

pub struct JobHandle<'a> {
    pub id: String,
    jobs: &'a Jobs,
    bus: &'a Bus,
}

impl Jobs {
    pub fn start<'a>(&'a self, bus: &'a Bus, kind: &str, title: &str) -> JobHandle<'a> {
        let j = JobInfo { id: new_id(), kind: kind.into(), title: title.into(), status: "running".into(), progress: None, message: String::new(), result: Value::Null, created_at: now() };
        let id = j.id.clone();
        bus.send(Event::Job { info: serde_json::to_value(&j).unwrap() });
        self.map.write().insert(id.clone(), j);
        // keep at most 100 jobs
        let mut m = self.map.write();
        if m.len() > 100 {
            let mut v: Vec<(i64, String)> = m.values().filter(|j| j.status != "running").map(|j| (j.created_at, j.id.clone())).collect();
            v.sort();
            for (_, id) in v.iter().take(m.len() - 100) {
                m.remove(id);
            }
        }
        drop(m);
        JobHandle { id, jobs: self, bus }
    }
    /// Handle for an existing job (to report progress from another task).
    pub fn handle<'a>(&'a self, bus: &'a Bus, id: &str) -> JobHandle<'a> {
        JobHandle { id: id.to_string(), jobs: self, bus }
    }
    pub fn list(&self) -> Vec<JobInfo> {
        let mut v: Vec<JobInfo> = self.map.read().values().cloned().collect();
        v.sort_by_key(|j| -j.created_at);
        v
    }
    pub fn get(&self, id: &str) -> Option<JobInfo> {
        self.map.read().get(id).cloned()
    }
}

impl JobHandle<'_> {
    fn update(&self, f: impl FnOnce(&mut JobInfo)) {
        let mut m = self.jobs.map.write();
        if let Some(j) = m.get_mut(&self.id) {
            f(j);
            self.bus.send(Event::Job { info: serde_json::to_value(&*j).unwrap() });
        }
    }
    pub fn progress(&self, p: Option<f64>, msg: impl Into<String>) {
        let msg = msg.into();
        self.update(|j| {
            j.progress = p;
            j.message = msg;
        });
    }
    pub fn done(&self, result: Value, msg: impl Into<String>) {
        let msg = msg.into();
        self.update(|j| {
            j.status = "done".into();
            j.progress = Some(1.0);
            j.result = result;
            j.message = msg;
        });
    }
    pub fn fail(&self, msg: impl Into<String>) {
        let msg = msg.into();
        self.update(|j| {
            j.status = "failed".into();
            j.message = msg;
        });
    }
}
