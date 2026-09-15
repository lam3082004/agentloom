//! Agent giả: cho phép test toàn bộ scheduler, graph động và protocol mutation
//! mà không tốn một token nào và không cần mạng.

use super::{AgentAdapter, AgentOutcome, AgentRequest};
use crate::event::{EventKind, EventLog};
use async_trait::async_trait;

#[derive(Default)]
pub struct FakeAdapter {
    /// Node có id nằm trong đây sẽ fail — để test nhánh hỏng.
    pub fail: Vec<String>,
}

#[async_trait]
impl AgentAdapter for FakeAdapter {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn binary(&self) -> &'static str {
        "true"
    }

    async fn run(&self, req: AgentRequest, log: &EventLog) -> anyhow::Result<AgentOutcome> {
        log.emit(
            Some(req.node.clone()),
            EventKind::AgentText {
                text: format!("fake chạy: {}", req.task),
            },
        );
        // Nếu task có ghi dòng mutation thì ghi ra file như agent thật sẽ làm.
        if let Some(rest) = req.task.split_once("EMIT:").map(|x| x.1.to_string()) {
            let p = req.cwd.join(crate::harness::MUTATION_FILE);
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).ok();
            }
            // Chỉ dòng đầu: phần còn lại là protocol block do harness nối thêm.
            // Kết dòng bằng '\n' như agent thật: orchestrator coi dòng cuối
            // chưa xuống dòng là dòng đang viết dở.
            let first = rest.lines().next().unwrap_or("").trim().to_string();
            std::fs::write(&p, format!("{first}\n"))?;
        }
        // Task chứa "FAIL" thì hỏng — để test nhánh lỗi mà không cần cấu hình.
        // WRITE:<file>:<noi dung> — để test được chuỗi node có thấy việc của
        // node trước hay không, mà không cần gọi agent thật.
        if let Some(rest) = req.task.split_once("WRITE:").map(|x| x.1.to_string()) {
            let spec = rest.lines().next().unwrap_or("").trim().to_string();
            if let Some((name, body)) = spec.split_once(':') {
                std::fs::write(req.cwd.join(name), body)?;
            }
        }
        let ok = !req.task.contains("FAIL") && !self.fail.iter().any(|f| f == req.node.as_str());
        Ok(AgentOutcome {
            session: Some(format!("fake-{}", req.node)),
            ok,
            summary: if ok { "xong".into() } else { "hỏng".into() },
            cost_usd: 0.0,
            tokens_in: 0,
            tokens_out: 0,
        })
    }
}
