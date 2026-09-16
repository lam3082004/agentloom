//! Hỏi lại agent của một node cũ.
//!
//! Node đã chạy xong để lại ba thứ trong event log: nó là agent nào, nó làm
//! việc trong worktree nào, và id session của nó. Có đủ ba thứ đó thì resume
//! đúng phiên cũ được — agent còn nguyên ngữ cảnh, không phải kể lại từ đầu.
//!
//! Việc moi ba thứ đó ra khỏi log nằm ở đây chứ không nằm trong CLI, vì cả
//! `agentloom ask` lẫn nút "hỏi lại" trên dashboard đều cần đúng một phép
//! ấy — hai bản sao là hai cách hiểu log khác nhau.

use crate::event::{Event, EventKind};
use crate::ids::NodeId;
use std::path::PathBuf;

/// Đủ thứ để resume một node cũ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub agent: String,
    pub cwd: PathBuf,
    pub session: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AskError {
    #[error("node '{0}' không có trong log của lượt chạy này")]
    NoNode(String),
    #[error("node '{0}' không có bản ghi agent trong log")]
    NoAgent(String),
    #[error("node '{0}' chưa chạy xong lần nào nên không có session để resume")]
    NoSession(String),
    #[error("node '{0}' không có workspace trong log")]
    NoWorkspace(String),
    #[error("worktree của node '{0}' đã bị xoá: {1}")]
    GoneWorkspace(String, PathBuf),
}

/// Tìm agent, worktree và session của `node` trong log đã đọc sẵn.
pub fn resolve(events: &[Event], node: &NodeId) -> Result<Target, AskError> {
    let ten = node.to_string();
    let (mut agent, mut cwd, mut session, mut found) = (None, None, None, false);
    for e in events {
        if e.node.as_ref() != Some(node) {
            continue;
        }
        found = true;
        match &e.kind {
            EventKind::NodeAdded { agent: a, .. } => agent = Some(a.clone()),
            // Node chỉ đổi worktree khi bị spawn lại; trong một lượt chạy,
            // dòng cuối là đúng.
            EventKind::Workspace { path, .. } => cwd = Some(PathBuf::from(path)),
            EventKind::NodeFinished { session: s, .. } if s.is_some() => session = s.clone(),
            _ => {}
        }
    }
    if !found {
        return Err(AskError::NoNode(ten));
    }
    let agent = agent.ok_or_else(|| AskError::NoAgent(ten.clone()))?;
    let session = session.ok_or_else(|| AskError::NoSession(ten.clone()))?;
    let cwd = cwd.ok_or_else(|| AskError::NoWorkspace(ten.clone()))?;
    if !cwd.exists() {
        return Err(AskError::GoneWorkspace(ten, cwd));
    }
    Ok(Target {
        agent,
        cwd,
        session,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::RunId;
    use chrono::Utc;

    fn ev(node: Option<&str>, kind: EventKind) -> Event {
        Event {
            seq: 1,
            at: Utc::now(),
            node: node.map(|n| NodeId::new(n).unwrap()),
            kind,
        }
    }

    fn day_du(cwd: &str) -> Vec<Event> {
        vec![
            ev(
                None,
                EventKind::RunStarted {
                    goal: "g".into(),
                    run: RunId::generate(),
                },
            ),
            ev(
                Some("a"),
                EventKind::NodeAdded {
                    title: "a".into(),
                    agent: "fake".into(),
                    deps: vec![],
                    by: crate::event::Origin::Plan,
                    model: None,
                },
            ),
            ev(
                Some("a"),
                EventKind::Workspace {
                    action: "worktree".into(),
                    path: cwd.into(),
                },
            ),
            ev(
                Some("a"),
                EventKind::NodeFinished {
                    ok: true,
                    cost_usd: 0.0,
                    tokens_in: 0,
                    tokens_out: 0,
                    summary: "xong".into(),
                    session: Some("sess-1".into()),
                },
            ),
        ]
    }

    #[test]
    fn lay_dung_agent_worktree_va_session() {
        let d = std::env::temp_dir();
        let t = resolve(&day_du(&d.to_string_lossy()), &NodeId::new("a").unwrap()).unwrap();
        assert_eq!((t.agent.as_str(), t.session.as_str()), ("fake", "sess-1"));
        assert_eq!(t.cwd, d);
    }

    #[test]
    fn thieu_gi_thi_noi_ro_thieu_cai_do() {
        let d = std::env::temp_dir().to_string_lossy().into_owned();
        let id = NodeId::new("a").unwrap();
        assert_eq!(
            resolve(&[], &id),
            Err(AskError::NoNode("a".into())),
            "log không có node"
        );

        let mut evs = day_du(&d);
        evs.retain(|e| !matches!(e.kind, EventKind::NodeFinished { .. }));
        assert_eq!(resolve(&evs, &id), Err(AskError::NoSession("a".into())));

        let mut evs = day_du(&d);
        evs.retain(|e| !matches!(e.kind, EventKind::Workspace { .. }));
        assert_eq!(resolve(&evs, &id), Err(AskError::NoWorkspace("a".into())));

        // Worktree đã bị dọn (ví dụ bằng `agentloom clean`).
        let mat = std::env::temp_dir().join(format!("ag-mat-{}", uuid::Uuid::new_v4()));
        let evs = day_du(&mat.to_string_lossy());
        assert_eq!(
            resolve(&evs, &id),
            Err(AskError::GoneWorkspace("a".into(), mat))
        );
    }

    /// Node bị spawn lại có nhiều dòng workspace — phải lấy dòng cuối.
    #[test]
    fn workspace_lay_dong_cuoi() {
        let d = std::env::temp_dir();
        let mut evs = day_du("/khong/co/that");
        evs.insert(
            3,
            ev(
                Some("a"),
                EventKind::Workspace {
                    action: "worktree".into(),
                    path: d.to_string_lossy().into_owned(),
                },
            ),
        );
        assert_eq!(resolve(&evs, &NodeId::new("a").unwrap()).unwrap().cwd, d);
    }
}
