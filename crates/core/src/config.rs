//! Plan tĩnh (điểm xuất phát) + trần chi phí.
//!
//! Plan chỉ là *hạt giống*: graph thật sẽ mọc thêm lúc chạy qua mutation.
//! Mọi trần đều nằm ở đây, không nằm ở model — đúng nguyên tắc "trần chi phí
//! thuộc harness".

use crate::graph::NodeSpec;
use crate::ids::NodeId;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub goal: String,
    #[serde(default, rename = "node")]
    pub nodes: Vec<NodeSpec>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PlanError {
    #[error("node '{0}' bị khai báo hai lần")]
    Duplicate(NodeId),
    #[error("node '{0}' phụ thuộc '{1}' nhưng plan không có node đó")]
    MissingDep(NodeId, NodeId),
    #[error("plan có chu trình phụ thuộc quanh node '{0}'")]
    Cycle(NodeId),
}

impl Plan {
    pub fn from_toml(s: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(s)?)
    }

    /// Bắt lỗi plan *trước* khi chạy. Không có bước này, `Graph::add` chỉ lặng
    /// lẽ từ chối node hỏng rồi lượt chạy vẫn báo "OK" — người viết plan
    /// tưởng việc đã xong trong khi chẳng node nào chạy.
    pub fn validate(&self) -> Result<(), PlanError> {
        let mut seen: HashSet<&NodeId> = HashSet::new();
        for n in &self.nodes {
            if !seen.insert(&n.id) {
                return Err(PlanError::Duplicate(n.id.clone()));
            }
        }
        let deps: HashMap<&NodeId, &Vec<NodeId>> =
            self.nodes.iter().map(|n| (&n.id, &n.deps)).collect();
        for n in &self.nodes {
            for d in &n.deps {
                if !deps.contains_key(d) {
                    return Err(PlanError::MissingDep(n.id.clone(), d.clone()));
                }
            }
        }
        // Dep trỏ tới node khai báo sau là hợp lệ, nên chu trình phải tìm bằng
        // duyệt thật chứ không suy ra từ thứ tự khai báo.
        let mut done: HashSet<&NodeId> = HashSet::new();
        let mut stack: HashSet<&NodeId> = HashSet::new();
        for n in &self.nodes {
            if let Some(id) = find_cycle(&n.id, &deps, &mut done, &mut stack) {
                return Err(PlanError::Cycle(id.clone()));
            }
        }
        Ok(())
    }
}

fn find_cycle<'a>(
    id: &'a NodeId,
    deps: &HashMap<&'a NodeId, &'a Vec<NodeId>>,
    done: &mut HashSet<&'a NodeId>,
    stack: &mut HashSet<&'a NodeId>,
) -> Option<&'a NodeId> {
    if stack.contains(id) {
        return Some(id);
    }
    if done.contains(id) {
        return None;
    }
    let (key, ds) = deps.get_key_value(id)?;
    stack.insert(key);
    for d in ds.iter() {
        if let Some(c) = find_cycle(d, deps, done, stack) {
            return Some(c);
        }
    }
    stack.remove(key);
    done.insert(key);
    None
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub max_parallel: usize,
    pub max_nodes: usize,
    pub max_total_usd: f64,
    pub node_timeout: Duration,
    pub permission_mode: String,
    /// Giữ branch của agent sau khi chạy xong, để review.
    pub keep_branches: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_parallel: 3,
            max_nodes: 64,
            max_total_usd: 20.0,
            node_timeout: Duration::from_secs(30 * 60),
            // Mặc định thận trọng. Chạy hoàn toàn không giám sát cần
            // bypassPermissions, và đó phải là lựa chọn có ý thức.
            permission_mode: "acceptEdits".into(),
            keep_branches: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_duoc_plan_toml() {
        let p = Plan::from_toml(
            r#"
goal = "thêm auth"

[[node]]
id = "scan"
title = "đọc codebase"
agent = "claude"
task = "Đọc src/ và mô tả kiến trúc."

[[node]]
id = "impl"
title = "cài đặt"
agent = "codex"
task = "Thêm middleware auth."
deps = ["scan"]
"#,
        )
        .unwrap();
        assert_eq!(p.goal, "thêm auth");
        assert_eq!(p.nodes.len(), 2);
        assert_eq!(p.nodes[1].deps[0].as_str(), "scan");
        // isolate mặc định là worktree, không cần khai báo.
        assert_eq!(p.nodes[0].isolate, crate::graph::Isolate::Worktree);
        p.validate().unwrap();
    }

    fn node(id: &str, deps: &[&str]) -> String {
        let d: Vec<String> = deps.iter().map(|x| format!("\"{x}\"")).collect();
        format!(
            "[[node]]\nid = \"{id}\"\ntitle = \"{id}\"\nagent = \"fake\"\ntask = \"t\"\ndeps = [{}]\n",
            d.join(", ")
        )
    }

    #[test]
    fn validate_bat_id_trung() {
        let p = Plan::from_toml(&format!(
            "goal = \"g\"\n{}{}",
            node("a", &[]),
            node("a", &[])
        ))
        .unwrap();
        assert_eq!(
            p.validate(),
            Err(PlanError::Duplicate(NodeId::new("a").unwrap()))
        );
    }

    #[test]
    fn validate_bat_dep_khong_ton_tai() {
        let p = Plan::from_toml(&format!("goal = \"g\"\n{}", node("a", &["ma"]))).unwrap();
        assert_eq!(
            p.validate(),
            Err(PlanError::MissingDep(
                NodeId::new("a").unwrap(),
                NodeId::new("ma").unwrap()
            ))
        );
    }

    #[test]
    fn validate_bat_chu_trinh() {
        let p = Plan::from_toml(&format!(
            "goal = \"g\"\n{}{}",
            node("a", &["b"]),
            node("b", &["a"])
        ))
        .unwrap();
        assert!(matches!(p.validate(), Err(PlanError::Cycle(_))));
    }

    #[test]
    fn dep_khai_bao_sau_van_hop_le() {
        // Người viết plan không có nghĩa vụ sắp xếp topo; chỉ chu trình mới sai.
        let p = Plan::from_toml(&format!(
            "goal = \"g\"\n{}{}",
            node("b", &["a"]),
            node("a", &[])
        ))
        .unwrap();
        p.validate().unwrap();
    }
}
