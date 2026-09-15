//! Adapter Codex CLI.
//!
//! Schema từ một lần chạy thật của codex-cli 0.153.2:
//!   {"type":"thread.started","thread_id":"01a0..."}
//!   {"type":"turn.started"}
//!   {"type":"item.completed","item":{"id":..,"type":"agent_message","text":".."}}
//!   {"type":"turn.completed","usage":{"input_tokens":..,"output_tokens":..}}
//!
//! Khác claude ở một điểm quan trọng: codex **không** trả tiền đã tiêu.
//! Ta chỉ có token, nên chi phí ở đây là ƯỚC LƯỢNG theo bảng giá cấu hình
//! được, và phải nói rõ là ước lượng thay vì trộn lẫn với số thật của claude.

use super::{AgentAdapter, AgentOutcome, AgentRequest};
use crate::event::{EventKind, EventLog};
use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// USD cho mỗi 1 triệu token. Chỉ để ước lượng, đổi qua config.
#[derive(Debug, Clone, Copy)]
pub struct Price {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

impl Default for Price {
    fn default() -> Self {
        Self {
            input_per_mtok: 1.25,
            output_per_mtok: 10.0,
        }
    }
}

pub struct CodexAdapter;

#[async_trait]
impl AgentAdapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }
    fn binary(&self) -> &'static str {
        "codex"
    }

    async fn run(&self, req: AgentRequest, log: &EventLog) -> anyhow::Result<AgentOutcome> {
        let mut cmd = Command::new("codex");
        cmd.arg("exec");
        // resume là subcommand, không phải flag — khác hẳn claude.
        if let Some(s) = &req.session {
            cmd.arg("resume").arg(s);
        }
        cmd.arg("--json")
            .arg("--skip-git-repo-check")
            .arg("--sandbox")
            .arg("workspace-write")
            .arg("-C")
            .arg(&req.cwd);
        if let Some(m) = &req.model {
            cmd.arg("--model").arg(m);
        }
        let prompt = if req.protocol.is_empty() {
            req.task.clone()
        } else {
            format!(
                "{}\n\n=== HƯỚNG DẪN CỦA HỆ ĐIỀU PHỐI ===\n{}",
                req.task, req.protocol
            )
        };
        cmd.arg(&prompt)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Nhóm tiến trình riêng — cùng lý do như claude: codex có thể đẻ
            // shell con, giết mỗi pid trực tiếp để mồ côi phần còn lại.
            .process_group(0);

        let mut child = cmd.spawn()?;
        // Lấy trước khi bị `wait`/`start_kill` làm mất.
        let pid = child.id();
        let stdout = child.stdout.take().expect("stdout đã piped");
        let stderr = child.stderr.take().expect("stderr đã piped");

        let log2 = log.clone();
        let node2 = req.node.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                if !l.trim().is_empty() {
                    log2.emit(
                        Some(node2.clone()),
                        EventKind::AgentRaw {
                            stream: "stderr".into(),
                            line: l,
                        },
                    );
                }
            }
        });

        let mut out = AgentOutcome {
            session: req.session.clone(),
            ok: true,
            ..Default::default()
        };
        let mut lines = BufReader::new(stdout).lines();
        let price = Price::default();

        let parse = async {
            while let Some(line) = lines.next_line().await? {
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    log.emit(
                        Some(req.node.clone()),
                        EventKind::AgentRaw {
                            stream: "stdout".into(),
                            line,
                        },
                    );
                    continue;
                };
                handle(&v, &req, log, &mut out, price);
            }
            Ok::<_, anyhow::Error>(())
        };

        let mut cancel_rx = req.cancel.clone();
        tokio::select! {
            r = parse => r?,
            _ = tokio::time::sleep(req.timeout) => {
                let _ = child.start_kill();
                if let Some(pid) = pid { super::kill_process_group(pid).await; }
                log.emit(
                    Some(req.node.clone()),
                    EventKind::Note {
                        text: format!("hết giờ sau {:?}, đã kill", req.timeout),
                    },
                );
                out.ok = false;
                out.summary = "timeout".into();
                return Ok(out);
            }
            _ = super::wait_for_cancel(&mut cancel_rx) => {
                let _ = child.start_kill();
                if let Some(pid) = pid { super::kill_process_group(pid).await; }
                log.emit(
                    Some(req.node.clone()),
                    EventKind::Note {
                        text: "bị huỷ — đã giết tiến trình codex".into(),
                    },
                );
                out.ok = false;
                out.summary = "huỷ theo yêu cầu người dùng".into();
                return Ok(out);
            }
        }

        let status = child.wait().await?;
        if !status.success() {
            out.ok = false;
            if out.summary.is_empty() {
                out.summary = format!("codex thoát với {status}");
            }
        }
        Ok(out)
    }
}

fn handle(v: &Value, req: &AgentRequest, log: &EventLog, out: &mut AgentOutcome, price: Price) {
    let node = Some(req.node.clone());
    match v.get("type").and_then(|t| t.as_str()) {
        Some("thread.started") => {
            if let Some(id) = v.get("thread_id").and_then(|s| s.as_str()) {
                out.session = Some(id.to_string());
            }
        }
        Some("item.completed") => {
            let item = v.get("item");
            let ty = item
                .and_then(|i| i.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let text = item
                .and_then(|i| i.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if ty == "agent_message" {
                if !text.trim().is_empty() {
                    out.summary = text.clone();
                    log.emit(node, EventKind::AgentText { text });
                }
            } else {
                // command_execution / file_change / reasoning / ... đều vào đây.
                log.emit(
                    node,
                    EventKind::AgentTool {
                        name: ty.to_string(),
                        detail: item.map(item_detail).unwrap_or_default(),
                    },
                );
            }
        }
        Some("turn.completed") => {
            let i = v
                .pointer("/usage/input_tokens")
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            let o = v
                .pointer("/usage/output_tokens")
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            out.tokens_in += i;
            out.tokens_out += o;
            out.cost_usd +=
                (i as f64 / 1e6) * price.input_per_mtok + (o as f64 / 1e6) * price.output_per_mtok;
        }
        Some("turn.failed") | Some("error") => {
            out.ok = false;
            let msg = v
                .pointer("/error/message")
                .and_then(|m| m.as_str())
                .or_else(|| v.get("message").and_then(|m| m.as_str()))
                .unwrap_or("codex báo lỗi");
            out.summary = msg.to_string();
            log.emit(
                node,
                EventKind::Note {
                    text: msg.to_string(),
                },
            );
        }
        _ => {}
    }
}

/// Rút gọn một item thành một dòng đọc được.
///
/// Codex có nhiều loại item với cấu trúc khác nhau; ta thử lần lượt các hình
/// dạng đã biết rồi mới bỏ cuộc, thay vì giả định đúng một dạng.
fn item_detail(item: &Value) -> String {
    // file_change có HAI dạng khác nhau và phải đỡ được cả hai:
    //   stream `--json`: "changes": [{"path": "...", "kind": "add"}]
    //   session rollout: "changes": {"/path": {"type": "add", "content": "..."}}
    // Chỉ hiện tên file, không bao giờ hiện nội dung — log không phải chỗ
    // để đổ cả file vào.
    if let Some(changes) = item.get("changes") {
        let mut names: Vec<String> = match changes {
            Value::Array(items) => items
                .iter()
                .filter_map(|c| c.get("path").and_then(|p| p.as_str()))
                .map(basename)
                .collect(),
            Value::Object(map) => map.keys().map(|p| basename(p)).collect(),
            _ => Vec::new(),
        };
        if !names.is_empty() {
            names.sort();
            names.dedup();
            return trim_line(&names.join(", "));
        }
    }
    for key in ["command", "text", "query", "url", "name"] {
        if let Some(s) = item.get(key).and_then(|x| x.as_str()) {
            if !s.trim().is_empty() {
                return trim_line(s);
            }
        }
    }
    // command_execution đôi khi mang command dưới dạng mảng argv.
    if let Some(arr) = item.get("command").and_then(|c| c.as_array()) {
        let joined: Vec<String> = arr
            .iter()
            .filter_map(|x| x.as_str())
            .map(|s| s.to_string())
            .collect();
        if !joined.is_empty() {
            return trim_line(&joined.join(" "));
        }
    }
    String::new()
}

fn basename(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

fn trim_line(s: &str) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() > 120 {
        format!("{}…", s.chars().take(120).collect::<String>())
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::NodeId;

    fn req() -> AgentRequest {
        AgentRequest {
            node: NodeId::new("n").unwrap(),
            task: "t".into(),
            protocol: String::new(),
            cwd: std::env::temp_dir(),
            session: None,
            model: None,
            permission_mode: "acceptEdits".into(),
            timeout: std::time::Duration::from_secs(1),
            cancel: tokio::sync::watch::channel(false).1,
        }
    }
    fn log() -> (EventLog, std::path::PathBuf) {
        let d = std::env::temp_dir().join(format!("ag-cx-{}", uuid::Uuid::new_v4()));
        (EventLog::create(d.join("e.jsonl")).unwrap(), d)
    }

    #[test]
    fn doc_dung_chuoi_su_kien_that_cua_codex() {
        let (l, d) = log();
        let mut out = AgentOutcome {
            ok: true,
            ..Default::default()
        };
        let r = req();
        for line in [
            r#"{"type":"thread.started","thread_id":"01a09f8d-0baf-7b80-a2af-db308b9d1227"}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"OK"}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":12518,"cached_input_tokens":8960,"output_tokens":5}}"#,
        ] {
            handle(
                &serde_json::from_str(line).unwrap(),
                &r,
                &l,
                &mut out,
                Price::default(),
            );
        }
        assert_eq!(
            out.session.as_deref(),
            Some("01a09f8d-0baf-7b80-a2af-db308b9d1227")
        );
        assert_eq!(out.summary, "OK");
        assert_eq!(out.tokens_in, 12518);
        assert_eq!(out.tokens_out, 5);
        // Ước lượng, không phải số do codex trả về.
        assert!(out.cost_usd > 0.0);
        assert!(out.ok);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn file_change_dang_stream_that() {
        // Nguyên văn từ `codex exec --json` 0.153.2. Dạng MẢNG.
        let item: Value = serde_json::from_str(
            r#"{"id":"item_1","type":"file_change","status":"completed",
                "changes":[{"path":"/tmp/cxprobe/z.txt","kind":"add"}]}"#,
        )
        .unwrap();
        assert_eq!(item_detail(&item), "z.txt");
    }

    #[test]
    fn file_change_dang_session_rollout() {
        // Cùng khái niệm nhưng là MAP — codex ghi session theo dạng khác hẳn
        // với dạng phát ra stream, nên adapter phải đỡ cả hai.
        let item: Value = serde_json::from_str(
            r#"{"id":"exec-1","type":"file_change","status":"completed","changes":{
                "/tmp/wt/greet.txt":{"type":"add","content":"XINCHAO\n"},
                "/tmp/wt/src/api.rs":{"type":"modify","content":"..."}}}"#,
        )
        .unwrap();
        assert_eq!(item_detail(&item), "api.rs, greet.txt");
    }

    #[test]
    fn command_execution_hien_lenh() {
        let item: Value =
            serde_json::from_str(r#"{"type":"command_execution","command":"cargo test -q"}"#)
                .unwrap();
        assert_eq!(item_detail(&item), "cargo test -q");
        let argv: Value =
            serde_json::from_str(r#"{"type":"command_execution","command":["ls","-la"]}"#).unwrap();
        assert_eq!(item_detail(&argv), "ls -la");
    }

    #[test]
    fn turn_failed_lam_node_that_bai() {
        let (l, d) = log();
        let mut out = AgentOutcome {
            ok: true,
            ..Default::default()
        };
        let line = r#"{"type":"turn.failed","error":{"message":"context limit"}}"#;
        handle(
            &serde_json::from_str(line).unwrap(),
            &req(),
            &l,
            &mut out,
            Price::default(),
        );
        assert!(!out.ok);
        assert_eq!(out.summary, "context limit");
        std::fs::remove_dir_all(d).ok();
    }
}
