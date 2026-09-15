//! Sự kiện là xương sống. Mọi thứ xảy ra đều thành một `Event`, được ghi
//! append-only ra JSONL và broadcast cho UI.
//!
//! Append-only là lựa chọn có chủ đích: log chạy được replay để dựng lại
//! đúng trạng thái cuối, nên TUI không phải là nguồn sự thật — nó chỉ là
//! một người đọc log.

use crate::ids::{NodeId, RunId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeId>,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    RunStarted {
        goal: String,
        run: RunId,
    },
    NodeAdded {
        title: String,
        agent: String,
        deps: Vec<NodeId>,
        by: Origin,
        /// Model agent dùng; `None` là mặc định của CLI agent. Log cũ không
        /// có trường này nên phải có `default` để còn replay được.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    NodeState {
        state: String,
    },
    /// Một dòng thô từ stdout/stderr của agent, giữ để debug.
    AgentRaw {
        stream: String,
        line: String,
    },
    /// Văn bản agent nói ra (đã bóc khỏi JSON).
    AgentText {
        text: String,
    },
    /// Agent gọi một tool.
    AgentTool {
        name: String,
        detail: String,
    },
    /// Agent kết thúc.
    NodeFinished {
        ok: bool,
        cost_usd: f64,
        tokens_in: u64,
        tokens_out: u64,
        summary: String,
        /// session_id (claude) / thread_id (codex) — cần để `agentloom ask`
        /// resume đúng phiên. `serde(default)`: log cũ chưa có trường này vẫn
        /// phải replay được, chỉ là không resume được node của lần chạy đó.
        #[serde(default)]
        session: Option<String>,
    },
    /// Một đề nghị sửa harness — ghi cả khi bị từ chối, kèm lý do.
    Mutation {
        op: String,
        target: String,
        accepted: bool,
        reason: String,
    },
    Workspace {
        action: String,
        path: String,
    },
    RunFinished {
        ok: bool,
        total_cost_usd: f64,
    },
    Note {
        text: String,
    },
}

/// Ai tạo ra node: người dùng viết plan, hay agent tự spawn lúc chạy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Plan,
    Agent,
}

/// Ghi JSONL + phát broadcast. Clone thoải mái, dùng chung một file handle.
#[derive(Clone)]
pub struct EventLog {
    inner: Arc<Inner>,
}

struct Inner {
    seq: Mutex<u64>,
    file: Mutex<std::fs::File>,
    path: PathBuf,
    tx: broadcast::Sender<Event>,
}

impl EventLog {
    pub fn create(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let (tx, _) = broadcast::channel(4096);
        Ok(Self {
            inner: Arc::new(Inner {
                seq: Mutex::new(0),
                file: Mutex::new(file),
                path,
                tx,
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.tx.subscribe()
    }

    pub fn emit(&self, node: Option<NodeId>, kind: EventKind) -> Event {
        let seq = {
            let mut s = self.inner.seq.lock().expect("seq poisoned");
            *s += 1;
            *s
        };
        let ev = Event {
            seq,
            at: Utc::now(),
            node,
            kind,
        };
        // Ghi đĩa trước, phát sau: nếu tiến trình chết ngay sau đó, cái đã
        // hiển thị cho người dùng chắc chắn cũng đã nằm trên đĩa.
        if let Ok(line) = serde_json::to_string(&ev) {
            if let Ok(mut f) = self.inner.file.lock() {
                let _ = writeln!(f, "{line}");
                let _ = f.flush();
            }
        }
        let _ = self.inner.tx.send(ev.clone());
        ev
    }

    /// Đọc lại toàn bộ log. Dòng hỏng bị bỏ qua chứ không làm hỏng replay.
    pub fn replay(path: impl AsRef<Path>) -> anyhow::Result<Vec<Event>> {
        let text = std::fs::read_to_string(path)?;
        Ok(text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Event>(l).ok())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghi_roi_doc_lai_giu_nguyen_thu_tu() {
        let dir = std::env::temp_dir().join(format!("ag-ev-{}", uuid::Uuid::new_v4()));
        let path = dir.join("events.jsonl");
        let log = EventLog::create(&path).unwrap();

        log.emit(
            None,
            EventKind::RunStarted {
                goal: "g".into(),
                run: RunId::generate(),
            },
        );
        let n = NodeId::new("a").unwrap();
        log.emit(
            Some(n.clone()),
            EventKind::NodeState {
                state: "running".into(),
            },
        );
        log.emit(
            Some(n),
            EventKind::AgentText {
                text: "xong".into(),
            },
        );

        let back = EventLog::replay(&path).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(
            back.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn dong_hong_khong_giet_replay() {
        let dir = std::env::temp_dir().join(format!("ag-ev2-{}", uuid::Uuid::new_v4()));
        let path = dir.join("events.jsonl");
        let log = EventLog::create(&path).unwrap();
        log.emit(None, EventKind::Note { text: "ok".into() });
        // Mô phỏng ghi dở do tiến trình bị giết giữa chừng.
        {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(f, "{{\"seq\": broken").unwrap();
        }
        let back = EventLog::replay(&path).unwrap();
        assert_eq!(back.len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }
}
