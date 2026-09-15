//! Graph động: node được thêm *trong lúc chạy*, không chỉ lúc khai báo.
//!
//! Vì agent được phép spawn node, mọi phép thêm phải tự bảo vệ: dep phải tồn
//! tại, không được tạo chu trình, và không được vượt trần số node. Một chu
//! trình lọt vào đây nghĩa là scheduler treo vĩnh viễn.

use crate::ids::NodeId;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    /// Chưa chạy, còn chờ dep.
    Blocked,
    /// Dep đã xong, sẵn sàng vào hàng đợi.
    Ready,
    Running,
    Done,
    Failed,
    Skipped,
}

impl NodeState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            NodeState::Done | NodeState::Failed | NodeState::Skipped
        )
    }
    pub fn as_str(self) -> &'static str {
        match self {
            NodeState::Blocked => "blocked",
            NodeState::Ready => "ready",
            NodeState::Running => "running",
            NodeState::Done => "done",
            NodeState::Failed => "failed",
            NodeState::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSpec {
    pub id: NodeId,
    pub title: String,
    /// Tên adapter: "claude" | "codex" | "fake".
    pub agent: String,
    pub task: String,
    #[serde(default)]
    pub deps: Vec<NodeId>,
    #[serde(default)]
    pub model: Option<String>,
    /// Lệnh shell chạy trong workspace sau khi agent xong. Khác 0 là node HỎNG,
    /// bất kể agent nói gì. Đây là chỗ duy nhất đóng khoảng cách
    /// generator–verifier: không có nó, "agent chạy xong" bị nhầm thành
    /// "việc đã xong".
    #[serde(default)]
    pub verify: Option<String>,
    /// Worktree riêng (mặc định) hay dùng chung cây thư mục gốc.
    #[serde(default)]
    pub isolate: Isolate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolate {
    #[default]
    Worktree,
    Shared,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub spec: NodeSpec,
    pub state: NodeState,
    /// session_id / thread_id của agent, để resume.
    pub session: Option<String>,
    pub cost_usd: f64,
    pub summary: String,
    pub workspace: Option<std::path::PathBuf>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GraphError {
    #[error("node '{0}' đã tồn tại")]
    Duplicate(NodeId),
    #[error("dep '{0}' không tồn tại")]
    MissingDep(NodeId),
    #[error("thêm '{0}' sẽ tạo chu trình")]
    Cycle(NodeId),
    #[error("đã chạm trần {0} node")]
    TooManyNodes(usize),
    #[error("node '{0}' không tồn tại")]
    NoSuchNode(NodeId),
}

#[derive(Debug)]
pub struct Graph {
    nodes: HashMap<NodeId, Node>,
    /// Giữ thứ tự thêm, để TUI hiển thị ổn định thay vì nhảy theo hash.
    order: Vec<NodeId>,
    max_nodes: usize,
}

impl Graph {
    pub fn new(max_nodes: usize) -> Self {
        Self {
            nodes: HashMap::new(),
            order: Vec::new(),
            max_nodes,
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn add(&mut self, spec: NodeSpec) -> Result<(), GraphError> {
        if self.nodes.contains_key(&spec.id) {
            return Err(GraphError::Duplicate(spec.id));
        }
        if self.nodes.len() >= self.max_nodes {
            return Err(GraphError::TooManyNodes(self.max_nodes));
        }
        for d in &spec.deps {
            if !self.nodes.contains_key(d) {
                return Err(GraphError::MissingDep(d.clone()));
            }
        }
        // Dep chỉ trỏ tới node đã có, nên chu trình không thể sinh ra ở đây.
        // Vẫn kiểm tra để phòng khi sau này có API đổi dep của node cũ.
        let id = spec.id.clone();
        let state = if spec.deps.is_empty() {
            NodeState::Ready
        } else {
            NodeState::Blocked
        };
        self.nodes.insert(
            id.clone(),
            Node {
                spec,
                state,
                session: None,
                cost_usd: 0.0,
                summary: String::new(),
                workspace: None,
            },
        );
        self.order.push(id.clone());
        if self.has_cycle() {
            self.nodes.remove(&id);
            self.order.retain(|x| x != &id);
            return Err(GraphError::Cycle(id));
        }
        Ok(())
    }

    fn has_cycle(&self) -> bool {
        let mut seen: HashSet<&NodeId> = HashSet::new();
        let mut stack: HashSet<&NodeId> = HashSet::new();
        fn go<'a>(
            g: &'a Graph,
            n: &'a NodeId,
            seen: &mut HashSet<&'a NodeId>,
            stack: &mut HashSet<&'a NodeId>,
        ) -> bool {
            if stack.contains(n) {
                return true;
            }
            if seen.contains(n) {
                return false;
            }
            seen.insert(n);
            stack.insert(n);
            if let Some(node) = g.nodes.get(n) {
                for d in &node.spec.deps {
                    if go(g, d, seen, stack) {
                        return true;
                    }
                }
            }
            stack.remove(n);
            false
        }
        self.order
            .iter()
            .any(|n| go(self, n, &mut seen, &mut stack))
    }

    pub fn get(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn set_state(&mut self, id: &NodeId, state: NodeState) -> Result<(), GraphError> {
        let n = self
            .nodes
            .get_mut(id)
            .ok_or_else(|| GraphError::NoSuchNode(id.clone()))?;
        n.state = state;
        Ok(())
    }

    pub fn node_mut(&mut self, id: &NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id)
    }

    /// Theo thứ tự thêm, để hiển thị và lập lịch đều tất định.
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.order.iter().filter_map(|i| self.nodes.get(i))
    }

    /// Cập nhật Blocked -> Ready cho node đã đủ dep; trả về danh sách vừa mở khoá.
    pub fn refresh_ready(&mut self) -> Vec<NodeId> {
        let mut opened = Vec::new();
        for id in self.order.clone() {
            let Some(n) = self.nodes.get(&id) else {
                continue;
            };
            if n.state != NodeState::Blocked {
                continue;
            }
            let deps = n.spec.deps.clone();
            let all_done = deps.iter().all(|d| {
                self.nodes
                    .get(d)
                    .map(|x| x.state == NodeState::Done)
                    .unwrap_or(false)
            });
            let any_bad = deps.iter().any(|d| {
                self.nodes
                    .get(d)
                    .map(|x| matches!(x.state, NodeState::Failed | NodeState::Skipped))
                    .unwrap_or(false)
            });
            if any_bad {
                // Dep hỏng thì nhánh phía sau không còn nghĩa — bỏ qua, đừng chạy mù.
                if let Some(n) = self.nodes.get_mut(&id) {
                    n.state = NodeState::Skipped;
                }
            } else if all_done {
                if let Some(n) = self.nodes.get_mut(&id) {
                    n.state = NodeState::Ready;
                }
                opened.push(id);
            }
        }
        opened
    }

    pub fn ready(&self) -> Vec<NodeId> {
        self.order
            .iter()
            .filter(|i| {
                self.nodes
                    .get(*i)
                    .map(|n| n.state == NodeState::Ready)
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    }

    pub fn all_settled(&self) -> bool {
        self.nodes.values().all(|n| n.state.is_terminal())
    }

    pub fn total_cost(&self) -> f64 {
        // fold từ 0.0 chứ không dùng `sum`: `sum` của f64 khởi tạo bằng -0.0
        // nên graph rỗng in ra "$-0.00".
        self.nodes.values().fold(0.0, |a, n| a + n.cost_usd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, deps: &[&str]) -> NodeSpec {
        NodeSpec {
            id: NodeId::new(id).unwrap(),
            title: id.into(),
            agent: "fake".into(),
            task: "t".into(),
            deps: deps.iter().map(|d| NodeId::new(*d).unwrap()).collect(),
            model: None,
            verify: None,
            isolate: Isolate::Shared,
        }
    }

    #[test]
    fn node_khong_dep_la_ready_ngay() {
        let mut g = Graph::new(10);
        g.add(spec("a", &[])).unwrap();
        assert_eq!(g.ready(), vec![NodeId::new("a").unwrap()]);
    }

    #[test]
    fn dep_chua_ton_tai_thi_tu_choi() {
        let mut g = Graph::new(10);
        assert_eq!(
            g.add(spec("b", &["a"])),
            Err(GraphError::MissingDep(NodeId::new("a").unwrap()))
        );
    }

    #[test]
    fn tran_so_node_chan_agent_spawn_vo_han() {
        let mut g = Graph::new(2);
        g.add(spec("a", &[])).unwrap();
        g.add(spec("b", &[])).unwrap();
        assert_eq!(g.add(spec("c", &[])), Err(GraphError::TooManyNodes(2)));
    }

    #[test]
    fn xong_dep_thi_mo_khoa_node_sau() {
        let mut g = Graph::new(10);
        g.add(spec("a", &[])).unwrap();
        g.add(spec("b", &["a"])).unwrap();
        assert_eq!(
            g.get(&NodeId::new("b").unwrap()).unwrap().state,
            NodeState::Blocked
        );
        g.set_state(&NodeId::new("a").unwrap(), NodeState::Done)
            .unwrap();
        let opened = g.refresh_ready();
        assert_eq!(opened, vec![NodeId::new("b").unwrap()]);
    }

    #[test]
    fn dep_hong_thi_bo_qua_nhanh_sau_chu_khong_chay_mu() {
        let mut g = Graph::new(10);
        g.add(spec("a", &[])).unwrap();
        g.add(spec("b", &["a"])).unwrap();
        g.set_state(&NodeId::new("a").unwrap(), NodeState::Failed)
            .unwrap();
        g.refresh_ready();
        assert_eq!(
            g.get(&NodeId::new("b").unwrap()).unwrap().state,
            NodeState::Skipped
        );
        assert!(g.all_settled());
    }

    #[test]
    fn graph_rong_co_chi_phi_duong_khong() {
        // -0.0 lọt ra ngoài thành "$-0.00" trên màn hình tổng kết.
        assert!(Graph::new(10).total_cost().is_sign_positive());
    }

    #[test]
    fn join_cho_tat_ca_nhanh() {
        let mut g = Graph::new(10);
        g.add(spec("a", &[])).unwrap();
        g.add(spec("b", &[])).unwrap();
        g.add(spec("j", &["a", "b"])).unwrap();
        g.set_state(&NodeId::new("a").unwrap(), NodeState::Done)
            .unwrap();
        assert!(
            g.refresh_ready().is_empty(),
            "mới xong 1 nhánh, join chưa được mở"
        );
        g.set_state(&NodeId::new("b").unwrap(), NodeState::Done)
            .unwrap();
        assert_eq!(g.refresh_ready(), vec![NodeId::new("j").unwrap()]);
    }
}
