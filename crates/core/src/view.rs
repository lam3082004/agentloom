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
    /// Như `started` nhưng serialize được: ảnh chụp `View` gửi cho trình duyệt
    /// phải mang nó, nếu không mở trang khi node đã chạy thì đồng hồ đứng im.
    pub started_ms: Option<i64>,
    pub elapsed_s: u64,
    pub lines: Vec<String>,
    pub model: Option<String>,
    /// Ai quyết định model: `user` (plan / dashboard), `agent` (agent cha chọn
    /// khi spawn — là `deps[0]`), `default` (không ai chọn, CLI tự dùng mặc định).
    /// Node con không kế thừa model của cha, nên cần nói rõ nguồn gốc.
    pub model_by: String,
    /// Thư mục agent đang làm việc (worktree riêng hoặc gốc repo).
    pub workspace: Option<String>,
    /// Việc gần nhất agent làm — một câu nói hoặc một lần gọi tool. Đây là
    /// thứ người xem graph muốn thấy trên node: "nó đang làm gì ngay lúc này".
    /// Không lấy ghi chú hệ thống (rate limit, verifier) vì chúng lặp liên tục
    /// và che mất hoạt động thật của agent.
    pub last: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// Một dòng trong luồng hoạt động chung của mọi agent — thứ dashboard dùng để
/// trả lời "chúng nó đang làm gì", xếp theo thời gian thật của event.
#[derive(Debug, Clone, Serialize)]
pub struct FeedItem {
    pub at_ms: i64,
    pub node: String,
    pub line: String,
}

/// Trần số dòng feed giữ lại. Feed nằm trong mọi ảnh chụp `View`; không có
/// trần thì phiên dài làm mỗi lần mở trang phải tải cả lịch sử.
const FEED_MAX: usize = 500;

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
    /// Mili-giây Unix lúc lượt chạy bắt đầu / kết thúc — cho đồng hồ tổng.
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub feed: Vec<FeedItem>,
    /// Thời điểm của event đang được fold; `push` dùng để đóng dấu feed.
    #[serde(skip)]
    now_ms: i64,
    /// Tổng số dòng từng vào feed, kể cả dòng đã bị cắt vì trần — để biết
    /// chính xác một event vừa thêm những dòng feed nào.
    #[serde(skip)]
    feed_seq: u64,
}

/// Những gì một event vừa thêm vào view — phần mặt web cần đẩy xuống.
#[derive(Debug, Clone, Default)]
pub struct Tracked {
    pub node: Option<String>,
    pub lines: Vec<String>,
    pub feed: Vec<FeedItem>,
}

impl View {
    /// Như `apply`, nhưng cho biết node nào vừa có thêm dòng log và thêm
    /// những dòng nào. Mặt web dùng cái này để client không phải tự diễn giải
    /// event — nó chỉ ghép dữ liệu đã fold sẵn, nên không có bản fold thứ hai
    /// để lệch.
    pub fn apply_tracked(&mut self, ev: &Event) -> Tracked {
        let id = ev.node.as_ref().map(|n| n.to_string());
        let before = id
            .as_ref()
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.lines.len())
            .unwrap_or(0);
        let seq_before = self.feed_seq;
        self.apply(ev);
        let lines = id
            .as_ref()
            .and_then(|i| self.nodes.get(i))
            .map(|n| n.lines[before.min(n.lines.len())..].to_vec())
            .unwrap_or_default();
        let moi = ((self.feed_seq - seq_before) as usize).min(self.feed.len());
        let feed = self.feed[self.feed.len() - moi..].to_vec();
        Tracked {
            node: id,
            lines,
            feed,
        }
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
                    model: n.model.clone(),
                    model_by: n.model_by.clone(),
                    workspace: n.workspace.clone(),
                    last: n.last.clone(),
                    summary: n.summary.clone(),
                    started_ms: n.started_ms,
                    tokens_in: n.tokens_in,
                    tokens_out: n.tokens_out,
                })
                .collect(),
            total_cost: self.total_cost,
            finished: self.finished,
            ok: self.ok,
            mutations_rejected: self.mutations_rejected,
            started_ms: self.started_ms,
            finished_ms: self.finished_ms,
        }
    }

    pub fn apply(&mut self, ev: &Event) {
        let node = ev.node.as_ref().map(|n| n.to_string());
        self.now_ms = ev.at.timestamp_millis();
        match &ev.kind {
            EventKind::RunStarted { goal, .. } => {
                self.goal = goal.clone();
                self.started_ms = Some(self.now_ms);
            }
            EventKind::NodeAdded {
                title,
                agent,
                deps,
                by,
                model,
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
                        started_ms: None,
                        elapsed_s: 0,
                        lines: Vec::new(),
                        model: model.clone(),
                        model_by: match (model, by) {
                            (None, _) => "default",
                            (Some(_), Origin::Plan) => "user",
                            (Some(_), Origin::Agent) => "agent",
                        }
                        .into(),
                        workspace: None,
                        last: String::new(),
                        tokens_in: 0,
                        tokens_out: 0,
                    },
                );
            }
            EventKind::NodeState { state } => {
                let bat_dau = state == "running";
                if let Some(n) = node.as_ref().and_then(|i| self.nodes.get_mut(i)) {
                    n.state = state.clone();
                    if state == "running" {
                        n.started = Some(ev.at);
                        n.started_ms = Some(ev.at.timestamp_millis());
                    }
                    if let Some(start) = n.started {
                        if state == "done" || state == "failed" || state == "skipped" {
                            n.elapsed_s = (ev.at - start).num_seconds().max(0) as u64;
                        }
                    }
                }
                if bat_dau {
                    self.push(node, "▶ bắt đầu chạy".into());
                }
            }
            EventKind::AgentText { text } => self.activity(node, format!("· {text}")),
            EventKind::AgentTool { name, detail } => {
                self.activity(node, format!("→ {name} {detail}"))
            }
            EventKind::AgentRaw { stream, line } => {
                if stream == "stderr" {
                    self.push(node, format!("! {line}"));
                }
            }
            EventKind::Workspace { action, path } => {
                if let Some(n) = node.as_ref().and_then(|i| self.nodes.get_mut(i)) {
                    n.workspace = Some(path.clone());
                }
                self.push(node, format!("⌂ {action}: {path}"))
            }
            EventKind::NodeFinished {
                ok,
                cost_usd,
                summary,
                tokens_in,
                tokens_out,
                ..
            } => {
                self.total_cost += cost_usd;
                if let Some(n) = node.as_ref().and_then(|i| self.nodes.get_mut(i)) {
                    n.cost = *cost_usd;
                    n.summary = summary.clone();
                    n.tokens_in = *tokens_in;
                    n.tokens_out = *tokens_out;
                }
                // Qua `push` để kết quả xong/hỏng vào cả feed, không chỉ log node.
                self.push(node, format!("{} {}", if *ok { "✓" } else { "✕" }, summary));
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
                self.finished_ms = Some(self.now_ms);
                self.total_cost = *total_cost_usd;
                self.ok = *ok;
            }
            EventKind::Note { text } => {
                self.notes.push(text.clone());
                self.push(node, format!("i {text}"));
            }
        }
    }

    /// Như `push`, nhưng đây là việc agent làm nên cũng cập nhật `last`.
    fn activity(&mut self, node: Option<String>, line: String) {
        if let Some(n) = node.as_ref().and_then(|i| self.nodes.get_mut(i)) {
            // Một dòng trên node graph: bỏ xuống dòng, cắt ngắn.
            let one = line.replace('\n', " ");
            n.last = if one.chars().count() > 90 {
                format!("{}…", one.chars().take(90).collect::<String>())
            } else {
                one
            };
        }
        self.push(node, line);
    }

    fn push(&mut self, node: Option<String>, line: String) {
        let Some(id) = node else { return };
        let Some(n) = self.nodes.get_mut(&id) else {
            return;
        };
        // Ghi chú rate limit đến sau MỖI lượt model trả lời — vào feed chung
        // thì nó nhấn chìm hoạt động thật của mọi agent. Vẫn giữ trong log node.
        let vao_feed = !line.starts_with("i rate limit");
        n.lines.push(line.clone());
        // Giữ trần bộ nhớ: phiên dài có thể sinh rất nhiều dòng.
        if n.lines.len() > 2000 {
            n.lines.drain(0..500);
        }
        if vao_feed {
            self.feed.push(FeedItem {
                at_ms: self.now_ms,
                node: id,
                line,
            });
            self.feed_seq += 1;
            if self.feed.len() > FEED_MAX {
                let du = self.feed.len() - FEED_MAX;
                self.feed.drain(0..du);
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
    pub ok: bool,
    pub mutations_rejected: usize,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
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
    pub model: Option<String>,
    pub model_by: String,
    pub workspace: Option<String>,
    pub last: String,
    pub summary: String,
    /// Mili-giây Unix lúc node bắt đầu chạy — để giao diện tự đếm giây khi node
    /// còn đang chạy (`elapsed_s` chỉ có sau khi node kết thúc).
    pub started_ms: Option<i64>,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// Một lần cập nhật đẩy xuống trình duyệt.
#[derive(Debug, Clone, Serialize)]
pub struct Patch {
    pub view: ViewMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    /// Dòng mới của luồng hoạt động chung — đã lọc sẵn ở server.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub feed: Vec<FeedItem>,
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
                model: None,
            },
        ));
        let t1 = v.apply_tracked(&ev(
            3,
            Some("a"),
            EventKind::AgentText {
                text: "một".into()
            },
        ));
        assert_eq!(t1.node.as_deref(), Some("a"));
        assert_eq!(t1.lines, vec!["· một".to_string()]);
        let t2 = v.apply_tracked(&ev(
            4,
            Some("a"),
            EventKind::AgentText { text: "hai".into() },
        ));
        assert_eq!(
            t2.lines,
            vec!["· hai".to_string()],
            "không được trả lại dòng cũ"
        );
        assert_eq!(v.nodes["a"].lines.len(), 2);
    }

    fn them_node(v: &mut View, id: &str) {
        v.apply(&ev(
            1,
            Some(id),
            EventKind::NodeAdded {
                title: id.into(),
                agent: "fake".into(),
                deps: vec![],
                by: Origin::Plan,
                model: None,
            },
        ));
    }

    /// Dashboard trả lời "chúng nó đang làm gì" bằng một luồng chung của MỌI
    /// agent. Kết quả xong/hỏng và lúc bắt đầu cũng phải vào đó — không chỉ
    /// dòng tool — nếu không feed kể chuyện thiếu đầu thiếu đuôi.
    #[test]
    fn feed_gom_hoat_dong_cua_moi_node_ke_ca_bat_dau_va_ket_qua() {
        let mut v = View::default();
        them_node(&mut v, "a");
        them_node(&mut v, "b");
        v.apply(&ev(
            2,
            Some("a"),
            EventKind::NodeState {
                state: "running".into(),
            },
        ));
        v.apply(&ev(
            3,
            Some("b"),
            EventKind::AgentTool {
                name: "Edit".into(),
                detail: "x.rs".into(),
            },
        ));
        v.apply(&ev(
            4,
            Some("a"),
            EventKind::NodeFinished {
                ok: true,
                cost_usd: 0.1,
                tokens_in: 1200,
                tokens_out: 34,
                summary: "xong việc".into(),
                session: None,
            },
        ));
        let dong: Vec<(String, String)> = v
            .feed
            .iter()
            .map(|f| (f.node.clone(), f.line.clone()))
            .collect();
        assert_eq!(
            dong,
            vec![
                ("a".to_string(), "▶ bắt đầu chạy".to_string()),
                ("b".to_string(), "→ Edit x.rs".to_string()),
                ("a".to_string(), "✓ xong việc".to_string()),
            ]
        );
        assert!(
            v.feed.iter().all(|f| f.at_ms > 0),
            "mỗi dòng phải có thời điểm"
        );
        assert_eq!(v.nodes["a"].tokens_in, 1200);
        assert_eq!(v.nodes["a"].tokens_out, 34);
    }

    #[test]
    fn rate_limit_khong_nhan_chim_feed_nhung_van_o_log_node() {
        let mut v = View::default();
        them_node(&mut v, "a");
        v.apply(&ev(
            2,
            Some("a"),
            EventKind::Note {
                text: "rate limit 5h đã dùng 0.4".into(),
            },
        ));
        assert!(v.feed.is_empty(), "{:?}", v.feed);
        assert_eq!(v.nodes["a"].lines.len(), 1);
    }

    #[test]
    fn feed_co_tran_va_apply_tracked_tra_dung_dong_moi_ke_ca_khi_bi_cat() {
        let mut v = View::default();
        them_node(&mut v, "a");
        for i in 0..(FEED_MAX + 50) {
            v.apply(&ev(
                2,
                Some("a"),
                EventKind::AgentText {
                    text: format!("dòng {i}"),
                },
            ));
        }
        assert_eq!(v.feed.len(), FEED_MAX);
        // Sau khi feed đã chạm trần, một event mới vẫn phải trả về đúng dòng của nó.
        let t = v.apply_tracked(&ev(
            3,
            Some("a"),
            EventKind::AgentText {
                text: "mới nhất".into(),
            },
        ));
        assert_eq!(t.feed.len(), 1);
        assert_eq!(t.feed[0].line, "· mới nhất");
        assert_eq!(v.feed.last().unwrap().line, "· mới nhất");
    }

    #[test]
    fn dong_ho_tong_cua_luot_chay() {
        let mut v = View::default();
        let t0 = Utc::now();
        let mut e1 = ev(
            1,
            None,
            EventKind::RunStarted {
                goal: "g".into(),
                run: crate::ids::RunId::generate(),
            },
        );
        e1.at = t0;
        v.apply(&e1);
        let mut e2 = ev(
            2,
            None,
            EventKind::RunFinished {
                ok: true,
                total_cost_usd: 0.0,
            },
        );
        e2.at = t0 + chrono::Duration::seconds(90);
        v.apply(&e2);
        let m = v.meta();
        assert_eq!(m.finished_ms.unwrap() - m.started_ms.unwrap(), 90_000);
        assert!(m.ok);
    }

    #[test]
    fn model_by_noi_ro_ai_chon_model() {
        let mut v = View::default();
        for (i, (id, by, model)) in [
            ("chinh", Origin::Plan, Some("opus")),
            ("con", Origin::Agent, Some("sonnet")),
            ("tron", Origin::Agent, None),
            ("plan-trong", Origin::Plan, None),
        ]
        .into_iter()
        .enumerate()
        {
            v.apply(&ev(
                i as u64,
                Some(id),
                EventKind::NodeAdded {
                    title: id.into(),
                    agent: "claude".into(),
                    deps: vec![],
                    by,
                    model: model.map(Into::into),
                },
            ));
        }
        let by: Vec<_> = v.meta().nodes.iter().map(|n| n.model_by.clone()).collect();
        assert_eq!(by, ["user", "agent", "default", "default"]);
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
                model: None,
            },
        ));
        v.apply(&ev(
            2,
            Some("a"),
            EventKind::AgentText {
                text: "dòng cũ".into(),
            },
        ));
        v.apply(&ev(
            3,
            Some("a"),
            EventKind::AgentTool {
                name: "Edit".into(),
                detail: "src/api.rs".into(),
            },
        ));
        let m = v.meta();
        assert_eq!(m.nodes.len(), 1);
        assert_eq!(m.nodes[0].agent, "claude");
        assert!(m.nodes[0].by_agent);
        // Được mang đúng một dòng hoạt động mới nhất để vẽ lên node...
        assert_eq!(m.nodes[0].last, "→ Edit src/api.rs");
        // ...nhưng không mang lịch sử log.
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            !json.contains("dòng cũ"),
            "meta không được mang theo lịch sử log"
        );
    }

    /// Mở trang web khi node ĐÃ đang chạy: trình duyệt nhận ảnh chụp `View` đầy
    /// đủ chứ không nhận patch, nên thời điểm bắt đầu phải nằm trong chính ảnh
    /// chụp — nếu không đồng hồ của node đang chạy đứng im.
    #[test]
    fn anh_chup_view_mang_thoi_diem_bat_dau_cua_node_dang_chay() {
        let mut v = View::default();
        v.apply(&ev(
            1,
            Some("a"),
            EventKind::NodeAdded {
                title: "a".into(),
                agent: "fake".into(),
                deps: vec![],
                by: Origin::Plan,
                model: None,
            },
        ));
        v.apply(&ev(
            2,
            Some("a"),
            EventKind::NodeState {
                state: "running".into(),
            },
        ));
        let json: serde_json::Value = serde_json::to_value(&v).unwrap();
        assert!(
            json["nodes"]["a"]["started_ms"].is_i64(),
            "ảnh chụp thiếu started_ms: {json}"
        );
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
                model: None,
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
