//! Protocol sửa harness lúc chạy.
//!
//! Agent con là tiến trình riêng, không gọi được vào bộ nhớ của ta. Kênh duy
//! nhất là một file JSONL trong workspace của nó: mỗi dòng là một đề nghị
//! sửa graph hoặc ghi skill/memory. Orchestrator tail file đó, **validate,
//! rồi mới áp**.
//!
//! Nguyên tắc: agent *đề nghị*, orchestrator *quyết định*. Mọi đề nghị đều
//! vào event log kèm accepted/lý do — kể cả cái bị từ chối, vì đó chính là
//! dữ liệu để biết prompt đang dạy agent làm sai điều gì.

use crate::graph::{Graph, Isolate, NodeSpec};
use crate::ids::NodeId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Đường dẫn tương đối trong workspace của node.
pub const MUTATION_FILE: &str = ".agentgraph/mutations.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Mutation {
    /// Thêm node mới vào graph khi đang chạy — fan-out động.
    Spawn {
        id: String,
        agent: String,
        task: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        after: Vec<String>,
        #[serde(default)]
        model: Option<String>,
        /// Lệnh kiểm chứng cho node con. Không có thì node con chỉ được tin
        /// lời agent — orchestrator vẫn cho chạy nhưng ghi rõ vào log.
        #[serde(default)]
        verify: Option<String>,
    },
    /// Ghi một skill để lần chạy sau dùng lại.
    WriteSkill { name: String, body: String },
    /// Ghi một mẩu memory bền.
    WriteMemory { key: String, value: String },
}

impl Mutation {
    pub fn op(&self) -> &'static str {
        match self {
            Mutation::Spawn { .. } => "spawn",
            Mutation::WriteSkill { .. } => "write_skill",
            Mutation::WriteMemory { .. } => "write_memory",
        }
    }
    pub fn target(&self) -> String {
        match self {
            Mutation::Spawn { id, .. } => id.clone(),
            Mutation::WriteSkill { name, .. } => name.clone(),
            Mutation::WriteMemory { key, .. } => key.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MutationError {
    #[error("id không hợp lệ: {0}")]
    BadId(String),
    #[error("agent '{0}' không có adapter")]
    UnknownAgent(String),
    #[error("task rỗng")]
    EmptyTask,
    #[error("verify rỗng — bỏ hẳn trường này nếu không có lệnh kiểm chứng")]
    EmptyVerify,
    #[error("graph từ chối: {0}")]
    Graph(String),
    #[error("tên skill/memory không hợp lệ: {0}")]
    BadName(String),
    #[error("node cha không được tự đặt mình làm dep")]
    SelfDep,
}

/// Tên skill/memory phải an toàn khi ghép vào đường dẫn.
fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Áp một mutation. `parent` là node đã phát ra nó — node mới mặc định phụ
/// thuộc cha, để nhánh động không chạy trước khi cha xong.
pub fn apply(
    m: &Mutation,
    parent: &NodeId,
    graph: &mut Graph,
    store: &HarnessStore,
) -> Result<(), MutationError> {
    match m {
        Mutation::Spawn {
            id,
            agent,
            task,
            title,
            after,
            model,
            verify,
        } => {
            let nid = NodeId::new(id.clone()).map_err(|e| MutationError::BadId(e.to_string()))?;
            if task.trim().is_empty() {
                return Err(MutationError::EmptyTask);
            }
            // Chuỗi rỗng không phải "không có verify" mà là một lệnh luôn xanh:
            // `sh -c ""` thoát 0. Chặn để agent không vô tình tắt verifier.
            if verify.as_deref().is_some_and(|v| v.trim().is_empty()) {
                return Err(MutationError::EmptyVerify);
            }
            if !crate::agent::known_agents().contains(&agent.as_str()) {
                return Err(MutationError::UnknownAgent(agent.clone()));
            }
            let mut deps = vec![parent.clone()];
            for a in after {
                let d = NodeId::new(a.clone()).map_err(|e| MutationError::BadId(e.to_string()))?;
                if d == nid {
                    return Err(MutationError::SelfDep);
                }
                if !deps.contains(&d) {
                    deps.push(d);
                }
            }
            graph
                .add(NodeSpec {
                    id: nid,
                    title: title.clone().unwrap_or_else(|| id.clone()),
                    agent: agent.clone(),
                    task: task.clone(),
                    deps,
                    model: model.clone(),
                    verify: verify.clone(),
                    isolate: Isolate::Worktree,
                })
                .map_err(|e| MutationError::Graph(e.to_string()))
        }
        Mutation::WriteSkill { name, body } => {
            if !safe_name(name) {
                return Err(MutationError::BadName(name.clone()));
            }
            store
                .write_skill(name, body)
                .map_err(|e| MutationError::Graph(e.to_string()))
        }
        Mutation::WriteMemory { key, value } => {
            if !safe_name(key) {
                return Err(MutationError::BadName(key.clone()));
            }
            store
                .write_memory(key, value)
                .map_err(|e| MutationError::Graph(e.to_string()))
        }
    }
}

/// Nơi skill và memory sống qua nhiều lần chạy.
#[derive(Debug, Clone)]
pub struct HarnessStore {
    root: PathBuf,
}

impl HarnessStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }
    pub fn memory_dir(&self) -> PathBuf {
        self.root.join("memory")
    }

    pub fn write_skill(&self, name: &str, body: &str) -> anyhow::Result<()> {
        let d = self.skills_dir();
        std::fs::create_dir_all(&d)?;
        std::fs::write(d.join(format!("{name}.md")), body)?;
        Ok(())
    }
    pub fn write_memory(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let d = self.memory_dir();
        std::fs::create_dir_all(&d)?;
        std::fs::write(d.join(format!("{key}.txt")), value)?;
        Ok(())
    }

    /// Skill đã tích luỹ, để chèn vào task của lần chạy sau.
    pub fn load_skills(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.skills_dir()) else {
            return v;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "md").unwrap_or(false) {
                if let (Some(stem), Ok(body)) = (p.file_stem(), std::fs::read_to_string(&p)) {
                    v.push((stem.to_string_lossy().into_owned(), body));
                }
            }
        }
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }
}

/// Đọc các dòng mutation mới kể từ `from_line`. Trả về (mutation, số dòng đã đọc).
/// Dòng hỏng bị bỏ qua nhưng vẫn tính, để không đọc lại vô hạn.
pub fn read_new(path: &Path, from_line: usize, still_running: bool) -> (Vec<Mutation>, usize) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (Vec::new(), from_line);
    };
    // Agent còn sống thì dòng cuối chưa có '\n' có thể đang viết dở: tính nó
    // là đã đọc thì phần còn lại sẽ không bao giờ được xét tới nữa. Khi agent
    // đã thoát, file không đổi nữa nên đọc hết.
    let text: &str = match (still_running, text.rsplit_once('\n')) {
        (true, Some((tron, _))) => tron,
        (true, None) => "",
        _ => &text,
    };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= from_line {
        return (Vec::new(), from_line);
    }
    let out = lines[from_line..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Mutation>(l).ok())
        .collect();
    (out, lines.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, NodeSpec};

    fn base_graph() -> (Graph, NodeId) {
        let mut g = Graph::new(10);
        let root = NodeId::new("root").unwrap();
        g.add(NodeSpec {
            id: root.clone(),
            title: "root".into(),
            agent: "fake".into(),
            task: "t".into(),
            deps: vec![],
            model: None,
            verify: None,
            isolate: Isolate::Shared,
        })
        .unwrap();
        (g, root)
    }

    fn store() -> (HarnessStore, PathBuf) {
        let d = std::env::temp_dir().join(format!("ag-hs-{}", uuid::Uuid::new_v4()));
        (HarnessStore::new(&d), d)
    }

    #[test]
    fn spawn_hop_le_them_node_phu_thuoc_cha() {
        let (mut g, root) = base_graph();
        let (s, d) = store();
        let m = Mutation::Spawn {
            id: "fix-flaky".into(),
            agent: "fake".into(),
            task: "sửa test flaky".into(),
            title: None,
            after: vec![],
            model: None,
            verify: None,
        };
        apply(&m, &root, &mut g, &s).unwrap();
        let n = g.get(&NodeId::new("fix-flaky").unwrap()).unwrap();
        assert_eq!(n.spec.deps, vec![root]);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn verify_cua_spawn_duoc_giu_lai_tren_node_moi() {
        let (mut g, root) = base_graph();
        let (s, d) = store();
        let m: Mutation = serde_json::from_str(
            r#"{"op":"spawn","id":"con","agent":"fake","task":"t","verify":"cargo test -q"}"#,
        )
        .unwrap();
        apply(&m, &root, &mut g, &s).unwrap();
        let n = g.get(&NodeId::new("con").unwrap()).unwrap();
        assert_eq!(n.spec.verify.as_deref(), Some("cargo test -q"));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn verify_rong_bi_tu_choi_vi_no_luon_xanh() {
        let (mut g, root) = base_graph();
        let (s, d) = store();
        let m: Mutation = serde_json::from_str(
            r#"{"op":"spawn","id":"con","agent":"fake","task":"t","verify":"  "}"#,
        )
        .unwrap();
        assert_eq!(
            apply(&m, &root, &mut g, &s),
            Err(MutationError::EmptyVerify)
        );
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn agent_khong_the_thoat_thu_muc_qua_id() {
        let (mut g, root) = base_graph();
        let (s, d) = store();
        let m = Mutation::Spawn {
            id: "../../../etc/cron.d/x".into(),
            agent: "fake".into(),
            task: "x".into(),
            title: None,
            after: vec![],
            model: None,
            verify: None,
        };
        assert!(matches!(
            apply(&m, &root, &mut g, &s),
            Err(MutationError::BadId(_))
        ));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn agent_khong_the_ghi_skill_ra_ngoai_store() {
        let (mut g, root) = base_graph();
        let (s, d) = store();
        let m = Mutation::WriteSkill {
            name: "../../evil".into(),
            body: "x".into(),
        };
        assert!(matches!(
            apply(&m, &root, &mut g, &s),
            Err(MutationError::BadName(_))
        ));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn agent_la_khong_duoc_chap_nhan() {
        let (mut g, root) = base_graph();
        let (s, d) = store();
        let m = Mutation::Spawn {
            id: "x".into(),
            agent: "skynet".into(),
            task: "t".into(),
            title: None,
            after: vec![],
            model: None,
            verify: None,
        };
        assert_eq!(
            apply(&m, &root, &mut g, &s),
            Err(MutationError::UnknownAgent("skynet".into()))
        );
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn tran_node_chan_spawn_chay_hoang() {
        let mut g = Graph::new(1); // đã có root là đầy
        let root = NodeId::new("root").unwrap();
        g.add(NodeSpec {
            id: root.clone(),
            title: "r".into(),
            agent: "fake".into(),
            task: "t".into(),
            deps: vec![],
            model: None,
            verify: None,
            isolate: Isolate::Shared,
        })
        .unwrap();
        let (s, d) = store();
        let m = Mutation::Spawn {
            id: "x".into(),
            agent: "fake".into(),
            task: "t".into(),
            title: None,
            after: vec![],
            model: None,
            verify: None,
        };
        assert!(matches!(
            apply(&m, &root, &mut g, &s),
            Err(MutationError::Graph(_))
        ));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn dong_viet_do_khong_bi_nuot_mat() {
        // Ta tail file trong lúc agent đang ghi. Tính dòng chưa xuống dòng là
        // đã đọc thì mutation đó biến mất vĩnh viễn.
        let d = std::env::temp_dir().join(format!("ag-mu2-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("m.jsonl");
        std::fs::write(&p, "{\"op\":\"write_memory\",\"key\":\"a\",\"val").unwrap();
        let (m1, c1) = read_new(&p, 0, true);
        assert!(m1.is_empty());
        std::fs::write(
            &p,
            "{\"op\":\"write_memory\",\"key\":\"a\",\"value\":\"1\"}\n",
        )
        .unwrap();
        let (m2, _) = read_new(&p, c1, true);
        assert_eq!(m2.len(), 1, "dòng viết xong phải được đọc");
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn skill_ghi_roi_doc_lai_duoc() {
        let (s, d) = store();
        s.write_skill("debug-perf", "1. grep handler").unwrap();
        let sk = s.load_skills();
        assert_eq!(sk.len(), 1);
        assert_eq!(sk[0].0, "debug-perf");
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn tail_chi_doc_dong_moi() {
        let d = std::env::temp_dir().join(format!("ag-mu-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("m.jsonl");
        std::fs::write(
            &p,
            "{\"op\":\"write_memory\",\"key\":\"a\",\"value\":\"1\"}\n",
        )
        .unwrap();
        let (m1, c1) = read_new(&p, 0, true);
        assert_eq!(m1.len(), 1);
        let (m2, c2) = read_new(&p, c1, true);
        assert!(m2.is_empty(), "đọc lại không được trả lại dòng cũ");
        assert_eq!(c1, c2);
        std::fs::remove_dir_all(d).ok();
    }
}
