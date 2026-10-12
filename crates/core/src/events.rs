use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Log { instance: String, stream: String, line: String, ts: i64 },
    Instance { info: Value },
    InstanceRemoved { id: String },
    System { snapshot: Value },
    Download { info: Value },
    Job { info: Value },
    Toast { level: String, title: String, message: String },
    Game { info: Value },
}

#[derive(Clone)]
pub struct Bus {
    tx: broadcast::Sender<Event>,
}

impl Default for Bus {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(4096);
        Self { tx }
    }
}

impl Bus {
    pub fn send(&self, e: Event) {
        let _ = self.tx.send(e);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
    pub fn toast(&self, level: &str, title: impl Into<String>, message: impl Into<String>) {
        self.send(Event::Toast { level: level.into(), title: title.into(), message: message.into() });
    }
}
