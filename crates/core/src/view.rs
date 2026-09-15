//! Trạng thái hiển thị, dựng hoàn toàn từ luồng event.
//!
//! Không mặt nào là nguồn sự thật — TUI, web và `replay` đều chỉ *fold* cùng
//! một event log qua đúng đoạn code này. Nhờ vậy ba mặt không thể lệch nhau,
//! và thêm mặt thứ tư chỉ là viết thêm phần vẽ.
//!
//! `Instant` không serialize được và web cần JSON, nên thời điểm bắt đầu lưu
//! bằng timestamp của chính event — cùng lúc cũng làm replay cho ra thời gian
//! đúng như lúc chạy thật, thay vì đo từ lúc mở lại log.

use crate::event::{Event, EventKind, Origin};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
pub struct NodeView {
    pub id: String,
    pub title: String,
    pub agent: String,
    pub state: String,
    pub deps: Vec<String>,
    pub by_agent: bool,
    pub cost: f64,
    pub summary: String,
    #[serde(skip)]
    pub started: Option<DateTime<Utc>>,
    pub elapsed_s: u64,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct View {
    pub goal: String,
    pub order: Vec<String>,
    pub nodes: HashMap<String, NodeView>,
    pub notes: Vec<String>,
    pub finished: bool,
    /// `ok` của `RunFinished` — chỉ có nghĩa khi `finished` là `true`. Lệnh
    /// `runs` cần cái này để phân biệt OK/CÓ LỖI mà không phải tự suy luận
    /// lại từ số node hỏng (View đã fold đúng một lần, dùng lại thay vì đếm
    /// lần hai).
    pub ok: bool,
    pub total_cost: f64,
    pub mutations_rejected: usize,
}

impl View {
    /// Như `apply`, nhưng cho biết node nào vừa có thêm dòng log và thêm
    /// những dòng nào. Mặt web dùng cái này để client không phải tự diễn giải
    /// event — nó chỉ ghép dữ liệu đã fold sẵn, nên không có bản fold thứ hai
    /// để lệch.
    pub fn apply_tracked(&mut self, ev: &Event) -> (Option<String>, Vec<String>) {
        let id = ev.node.as_ref().map(|n| n.to_string());
        let before = id
            .as_ref()
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.lines.len())
            .unwrap_or(0);
        self.apply(ev);
        let appended = id
            .as_ref()
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.lines[before.min(n.lines.len())..].to_vec())
            .unwrap_or_default();
        (id, appended)
    }

    /// Ảnh chụp gọn: mọi thứ trừ log. Đủ để vẽ danh sách node và thanh
    /// trạng thái, mà không phải đẩy lại toàn bộ log sau mỗi event.
    pub fn meta(&self) -> ViewMeta {
        ViewMeta {
            goal: self.goal.clone(),
            order: self.order.clone(),
            nodes: self
                .order
                .iter()
                .filter_map(|i| self.nodes.get(i))
                .map(|n| NodeMeta {
                    id: n.id.clone(),
                    title: n.title.clone(),
                    agent: n.agent.clone(),
                    state: n.state.clone(),
                    deps: n.deps.clone(),
                    by_agent: n.by_agent,
                    cost: n.cost,
                    elapsed_s: n.elapsed_s,
                })
                .collect(),
            total_cost: self.total_cost,
            finished: self.finished,
            mutations_rejected: self.mutations_rejected,
        }
    }

    pub fn apply(&mut self, ev: &Event) {
        let node = ev.node.as_ref().map(|n| n.to_string());
        match &ev.kind {
            EventKind::RunStarted { goal, .. } => self.goal = goal.clone(),
            EventKind::NodeAdded {
                title,
                agent,
                deps,
                by,
            } => {
                let Some(id) = node else { return };
                if !self.nodes.contains_key(&id) {
                    self.order.push(id.clone());
                }
                self.nodes.insert(
                    id.clone(),
                    NodeView {
                        id,
                        title: title.clone(),
                        agent: agent.clone(),
                        state: "blocked".into(),
                        deps: deps.iter().map(|d| d.to_string()).collect(),
                        by_agent: *by == Origin::Agent,
                        cost: 0.0,
                        summary: String::new(),
                        started: None,
                        elapsed_s: 0,
                        lines: Vec::new(),
                    },
                );
            }
            EventKind::NodeState { state } => {
                if let Some(n) = node.and_then(|i| self.nodes.get_mut(&i)) {
                    n.state = state.clone();
                    if state == "running" {
                        n.started = Some(ev.at);
                    }
                    if let Some(start) = n.started {
                        if state == "done" || state == "failed" || state == "skipped" {
                            n.elapsed_s = (ev.at - start).num_seconds().max(0) as u64;
                        }
                    }
                }
            }
            EventKind::AgentText { text } => self.push(node, format!("· {text}")),
            EventKind::AgentTool { name, detail } => self.push(node, format!("→ {name} {detail}")),
            EventKind::AgentRaw { stream, line } => {
                if stream == "stderr" {
                    self.push(node, format!("! {line}"));
                }
            }
            EventKind::Workspace { action, path } => self.push(node, format!("⌂ {action}: {path}")),
            EventKind::NodeFinished {
                ok,
                cost_usd,
                summary,
                ..
            } => {
                self.total_cost += cost_usd;
                if let Some(n) = node.and_then(|i| self.nodes.get_mut(&i)) {
                    n.cost = *cost_usd;
                    n.summary = summary.clone();
                    n.lines
                        .push(format!("{} {}", if *ok { "✓" } else { "✕" }, summary));
                }
            }
            EventKind::Mutation {
                op,
                target,
                accepted,
                reason,
            } => {
                if !accepted {
                    self.mutations_rejected += 1;
                }
                let msg = if *accepted {
                    format!("⚙ {op} {target} — chấp nhận")
                } else {
                    format!("⚙ {op} {target} — TỪ CHỐI: {reason}")
                };
                self.push(node, msg);
            }
            EventKind::RunFinished { total_cost_usd, ok } => {
                self.finished = true;
                self.total_cost = *total_cost_usd;
                self.ok = *ok;
            }
            EventKind::Note { text } => {
                self.notes.push(text.clone());
                self.push(node, format!("i {text}"));
            }
        }
    }

    fn push(&mut self, node: Option<String>, line: String) {
        if let Some(n) = node.and_then(|i| self.nodes.get_mut(&i)) {
            n.lines.push(line);
            // Giữ trần bộ nhớ: phiên dài có thể sinh rất nhiều dòng.
            if n.lines.len() > 2000 {
                n.lines.drain(0..500);
            }
        }
    }

    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut run = 0;
        let mut done = 0;
        let mut fail = 0;
        let mut wait = 0;
        for n in self.nodes.values() {
            match n.state.as_str() {
                "running" => run += 1,
                "done" => done += 1,
                "failed" => fail += 1,
                _ => wait += 1,
            }
        }
        (run, done, fail, wait)
    }
}

/// Ảnh chụp trạng thái không kèm log, dùng cho cập nhật thời gian thực.
#[derive(Debug, Clone, Serialize)]
pub struct ViewMeta {
    pub goal: String,
    pub order: Vec<String>,
    pub nodes: Vec<NodeMeta>,
    pub total_cost: f64,
    pub finished: bool,
    pub mutations_rejected: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeMeta {
    pub id: String,
    pub title: String,
    pub agent: String,
    pub state: String,
    pub deps: Vec<String>,
    pub by_agent: bool,
    pub cost: f64,
    pub elapsed_s: u64,
}

/// Một lần cập nhật đẩy xuống trình duyệt.
#[derive(Debug, Clone, Serialize)]
pub struct Patch {
    pub view: ViewMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, Origin};
    use crate::ids::{NodeId, RunId};
    use chrono::Utc;

    fn ev(seq: u64, node: Option<&str>, kind: EventKind) -> Event {
        Event {
            seq,
            at: Utc::now(),
            node: node.map(|n| NodeId::new(n).unwrap()),
            kind,
        }
    }

    #[test]
    fn apply_tracked_chi_tra_ve_dong_vua_them() {
        let mut v = View::default();
        v.apply(&ev(
            1,
            None,
            EventKind::RunStarted {
                goal: "g".into(),
                run: RunId::generate(),
            },
        ));
        v.apply(&ev(
            2,
            Some("a"),
            EventKind::NodeAdded {
                title: "a".into(),
                agent: "fake".into(),
                deps: vec![],
                by: Origin::Plan,
            },
        ));
        let (n1, l1) = v.apply_tracked(&ev(
            3,
            Some("a"),
            EventKind::AgentText {
                text: "một".into()
            },
        ));
        assert_eq!(n1.as_deref(), Some("a"));
        assert_eq!(l1, vec!["· một".to_string()]);
        let (_, l2) = v.apply_tracked(&ev(
            4,
            Some("a"),
            EventKind::AgentText { text: "hai".into() },
        ));
        assert_eq!(l2, vec!["· hai".to_string()], "không được trả lại dòng cũ");
        assert_eq!(v.nodes["a"].lines.len(), 2);
    }

    #[test]
    fn meta_bo_log_nhung_giu_du_thong_tin_ve_node() {
        let mut v = View::default();
        v.apply(&ev(
            1,
            Some("a"),
            EventKind::NodeAdded {
                title: "tiêu đề".into(),
                agent: "claude".into(),
                deps: vec![],
                by: Origin::Agent,
            },
        ));
        v.apply(&ev(
            2,
            Some("a"),
            EventKind::AgentText {
                text: "dài".into()
            },
        ));
        let m = v.meta();
        assert_eq!(m.nodes.len(), 1);
        assert_eq!(m.nodes[0].agent, "claude");
        assert!(m.nodes[0].by_agent);
        let json = serde_json::to_string(&m).unwrap();
        assert!(!json.contains("dài"), "meta không được mang theo log");
    }

    #[test]
    fn elapsed_tinh_theo_thoi_gian_event_nen_replay_ra_dung_so() {
        let mut v = View::default();
        let t0 = Utc::now();
        let mut e1 = ev(
            1,
            Some("a"),
            EventKind::NodeAdded {
                title: "a".into(),
                agent: "fake".into(),
                deps: vec![],
                by: Origin::Plan,
            },
        );
        e1.at = t0;
        v.apply(&e1);
        let mut e2 = ev(
            2,
            Some("a"),
            EventKind::NodeState {
                state: "running".into(),
            },
        );
        e2.at = t0;
        v.apply(&e2);
        let mut e3 = ev(
            3,
            Some("a"),
            EventKind::NodeState {
                state: "done".into(),
            },
        );
        e3.at = t0 + chrono::Duration::seconds(42);
        v.apply(&e3);
        assert_eq!(
            v.nodes["a"].elapsed_s, 42,
            "replay phải ra đúng 42s như lúc chạy thật"
        );
    }
}
