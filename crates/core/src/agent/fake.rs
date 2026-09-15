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
        // Adapter thật không bao giờ log nguyên văn protocol (chỉ nối vào
        // system prompt của tiến trình con); fake ghi lại để test xác nhận
        // nội dung operator gửi tới agent — ví dụ cảnh báo merge đụng độ —
        // mà không cần đọc bộ nhớ trong của claude/codex thật.
        if !req.protocol.is_empty() {
            log.emit(
                Some(req.node.clone()),
                EventKind::Note {
                    text: format!("[fake:protocol] {}", req.protocol),
                },
            );
        }
        // Nếu task có ghi dòng mutation thì ghi ra file như agent thật sẽ làm.
        if let Some(rest) = req.task.split_once("EMIT:").map(|x| x.1.to_string()) {
            let p = req.cwd.join(crate::harness::MUTATION_FILE);
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).ok();
            }
            // Lấy mọi dòng JSON liên tiếp sau `EMIT:` — một agent thật có thể
            // spawn nhiều node con cùng lúc. Dừng ở dòng đầu không phải JSON
            // để các lệnh khác trong task (`SLEEP:`...) không lẫn vào.
            // Kết mỗi dòng bằng '\n' như agent thật: orchestrator coi dòng cuối
            // chưa xuống dòng là dòng đang viết dở.
            let lines: Vec<&str> = rest
                .lines()
                .map(str::trim)
                .take_while(|l| l.starts_with('{'))
                .collect();
            std::fs::write(
                &p,
                lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
            )?;
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
        // SLEEP:<giay> — spawn một tiến trình `sleep` THẬT, trong process
        // group riêng như claude/codex thật, để test huỷ giữa chừng kiểm
        // được bằng PID thật thay vì chỉ giả lập trong bộ nhớ Rust.
        if let Some(rest) = req.task.split_once("SLEEP:").map(|x| x.1.to_string()) {
            let secs: u64 = rest
                .lines()
                .next()
                .unwrap_or("0")
                .trim()
                .parse()
                .unwrap_or(0);
            let mut cmd = tokio::process::Command::new("sleep");
            cmd.arg(secs.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            super::own_process_group(&mut cmd);
            let mut child = cmd.spawn()?;
            let pid = child.id();
            if let Some(pid) = pid {
                log.emit(
                    Some(req.node.clone()),
                    EventKind::Note {
                        text: format!("fake-pid:{pid}"),
                    },
                );
            }
            let mut cancel_rx = req.cancel.clone();
            tokio::select! {
                _ = child.wait() => {}
                _ = tokio::time::sleep(req.timeout) => {
                    // Giết cả cây TRƯỚC khi giết agent: agent chết là con của nó bị chuyển
                    // về init, mất dấu cha–con và không còn lần ra để giết.
                    if let Some(pid) = pid { super::kill_process_group(pid).await; }
                    let _ = child.start_kill();
                    return Ok(AgentOutcome{ ok: false, summary: "timeout".into(), ..Default::default() });
                }
                _ = super::wait_for_cancel(&mut cancel_rx) => {
                    // Giết cả cây TRƯỚC khi giết agent: agent chết là con của nó bị chuyển
                    // về init, mất dấu cha–con và không còn lần ra để giết.
                    if let Some(pid) = pid { super::kill_process_group(pid).await; }
                    let _ = child.start_kill();
                    return Ok(AgentOutcome{
                        session: Some(format!("fake-{}", req.node)),
                        ok: false,
                        summary: "huỷ theo yêu cầu người dùng".into(),
                        ..Default::default()
                    });
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::NodeId;

    /// Bug thật bắt được khi chạy `agentloom ask` với claude thật: request
    /// dùng kênh huỷ dùng-một-lần (`watch::channel(false).1`, `Sender` bị rớt
    /// ngay lập tức vì không ai giữ). `cancel_rx.changed()` trả `Err` tức
    /// khắc trong trường hợp đó — nếu `select!` coi mọi lần `changed()` hoàn
    /// thành (kể cả lỗi) là "đã huỷ" thì request chưa từng được gửi `true`
    /// vẫn bị coi là bị huỷ ngay khi vừa bắt đầu.
    #[tokio::test]
    async fn khong_ai_giu_sender_thi_khong_duoc_coi_la_da_huy() {
        let d = std::env::temp_dir().join(format!("ag-fk-{}", uuid::Uuid::new_v4()));
        let log = EventLog::create(d.join("e.jsonl")).unwrap();
        let req = AgentRequest {
            node: NodeId::new("n").unwrap(),
            task: "SLEEP:1".into(),
            protocol: String::new(),
            cwd: std::env::temp_dir(),
            session: None,
            model: None,
            permission_mode: "acceptEdits".into(),
            timeout: std::time::Duration::from_secs(5),
            // Sender tạm, rớt ngay khi hết dòng này — mô phỏng đúng
            // `agentloom ask`, nơi không ai cần huỷ nên không giữ sender.
            cancel: tokio::sync::watch::channel(false).1,
        };
        let out = FakeAdapter::default().run(req, &log).await.unwrap();
        assert!(
            out.ok,
            "không ai gửi huỷ thì không được coi là huỷ: {out:?}"
        );
        std::fs::remove_dir_all(d).ok();
    }
}
