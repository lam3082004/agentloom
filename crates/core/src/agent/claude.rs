//! Adapter Claude Code.
//!
//! Schema dưới đây lấy từ một lần chạy thật của claude 2.1.270, không phải
//! đoán. Các dòng quan tâm:
//!   {"type":"system","subtype":"init","session_id":...}
//!   {"type":"assistant","message":{"content":[{"type":"text"|"tool_use",...}]}}
//!   {"type":"result","total_cost_usd":..,"is_error":..,"result":..,"usage":{..}}
//!   {"type":"rate_limit_event","rate_limit_info":{..}}

use super::{AgentAdapter, AgentOutcome, AgentRequest};
use crate::event::{EventKind, EventLog};
use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

pub struct ClaudeAdapter;

#[async_trait]
impl AgentAdapter for ClaudeAdapter {
    fn name(&self) -> &'static str {
        "claude"
    }
    fn binary(&self) -> &'static str {
        "claude"
    }

    async fn run(&self, req: AgentRequest, log: &EventLog) -> anyhow::Result<AgentOutcome> {
        let mut cmd = Command::new("claude");
        cmd.arg("-p")
            .arg(&req.task)
            .arg("--output-format")
            .arg("stream-json")
            // stream-json chỉ phát đủ event khi có --verbose.
            .arg("--verbose")
            .arg("--permission-mode")
            .arg(&req.permission_mode)
            .current_dir(&req.cwd)
            // Không có stdin thì claude chờ 3s rồi cảnh báo; đóng hẳn cho sạch.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if !req.protocol.is_empty() {
            cmd.arg("--append-system-prompt").arg(&req.protocol);
        }
        if let Some(m) = &req.model {
            cmd.arg("--model").arg(m);
        }
        if let Some(s) = &req.session {
            cmd.arg("--resume").arg(s);
        }

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("stdout đã piped");
        let stderr = child.stderr.take().expect("stderr đã piped");

        let log2 = log.clone();
        let node2 = req.node.clone();
        // stderr chạy nền: không được để nó lấp đầy pipe rồi khoá tiến trình con.
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
            ..Default::default()
        };
        let mut lines = BufReader::new(stdout).lines();

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
                handle(&v, &req, log, &mut out);
            }
            Ok::<_, anyhow::Error>(())
        };

        match tokio::time::timeout(req.timeout, parse).await {
            Ok(r) => r?,
            Err(_) => {
                let _ = child.start_kill();
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
        }

        let status = child.wait().await?;
        // `result.is_error` là nguồn chính; exit code chỉ bắt trường hợp chết sớm.
        if !status.success() && out.summary.is_empty() {
            out.ok = false;
            out.summary = format!("claude thoát với {status}");
        }
        Ok(out)
    }
}

fn handle(v: &Value, req: &AgentRequest, log: &EventLog, out: &mut AgentOutcome) {
    let node = Some(req.node.clone());
    match v.get("type").and_then(|t| t.as_str()) {
        Some("system") => {
            if let Some(sid) = v.get("session_id").and_then(|s| s.as_str()) {
                out.session = Some(sid.to_string());
            }
        }
        Some("assistant") => {
            if let Some(sid) = v.get("session_id").and_then(|s| s.as_str()) {
                out.session = Some(sid.to_string());
            }
            let content = v.pointer("/message/content").and_then(|c| c.as_array());
            for block in content.into_iter().flatten() {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                            if !t.trim().is_empty() {
                                log.emit(node.clone(), EventKind::AgentText { text: t.into() });
                            }
                        }
                    }
                    Some("tool_use") => {
                        let name = block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        log.emit(
                            node.clone(),
                            EventKind::AgentTool {
                                name,
                                detail: tool_detail(block),
                            },
                        );
                    }
                    _ => {}
                }
            }
        }
        Some("rate_limit_event") => {
            if let Some(u) = v.pointer("/rate_limit_info/unifiedWindows/five_hour/utilization") {
                log.emit(
                    node,
                    EventKind::Note {
                        text: format!("rate limit 5h đã dùng {u}"),
                    },
                );
            }
        }
        Some("result") => {
            if let Some(sid) = v.get("session_id").and_then(|s| s.as_str()) {
                out.session = Some(sid.to_string());
            }
            out.cost_usd = v
                .get("total_cost_usd")
                .and_then(|c| c.as_f64())
                .unwrap_or(0.0);
            out.tokens_in = v
                .pointer("/usage/input_tokens")
                .and_then(|t| t.as_u64())
                .unwrap_or(0)
                + v.pointer("/usage/cache_read_input_tokens")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0);
            out.tokens_out = v
                .pointer("/usage/output_tokens")
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            if let Some(d) = v.get("permission_denials").and_then(|d| d.as_array()) {
                if !d.is_empty() {
                    let names: Vec<String> = d
                        .iter()
                        .filter_map(|x| x.get("tool_name").and_then(|t| t.as_str()))
                        .map(|s| s.to_string())
                        .collect();
                    log.emit(
                        node.clone(),
                        EventKind::Note {
                            text: format!(
                                "agent bị chặn quyền ({} lần): {} — verifier sẽ quyết định",
                                d.len(),
                                names.join(", ")
                            ),
                        },
                    );
                }
            }
            let is_err = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
            out.ok = !is_err;
            out.summary = v
                .get("result")
                .and_then(|r| r.as_str())
                .unwrap_or(if is_err { "lỗi" } else { "xong" })
                .to_string();
        }
        _ => {}
    }
}

/// Rút gọn tham số tool thành một dòng đọc được trong TUI.
fn tool_detail(block: &Value) -> String {
    let input = block.get("input");
    for key in [
        "file_path",
        "command",
        "pattern",
        "path",
        "url",
        "description",
    ] {
        if let Some(s) = input.and_then(|i| i.get(key)).and_then(|s| s.as_str()) {
            let s = s.replace('\n', " ");
            return if s.chars().count() > 120 {
                format!("{}…", s.chars().take(120).collect::<String>())
            } else {
                s
            };
        }
    }
    String::new()
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
        }
    }

    fn log() -> (EventLog, std::path::PathBuf) {
        let d = std::env::temp_dir().join(format!("ag-cc-{}", uuid::Uuid::new_v4()));
        let p = d.join("e.jsonl");
        (EventLog::create(&p).unwrap(), d)
    }

    /// Dòng `result` thật, cắt từ lần chạy claude 2.1.270.
    const RESULT_LINE: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"OK","session_id":"aa419a3b-1d2e-4570-a720-5ea09571489b","total_cost_usd":0.050725,"num_turns":1,"usage":{"input_tokens":2,"cache_read_input_tokens":16680,"output_tokens":4}}"#;

    #[test]
    fn boc_dung_cost_session_va_ket_qua_tu_dong_result_that() {
        let (l, d) = log();
        let mut out = AgentOutcome::default();
        handle(
            &serde_json::from_str(RESULT_LINE).unwrap(),
            &req(),
            &l,
            &mut out,
        );
        assert!(out.ok);
        assert_eq!(out.summary, "OK");
        assert!((out.cost_usd - 0.050725).abs() < 1e-9);
        assert_eq!(
            out.session.as_deref(),
            Some("aa419a3b-1d2e-4570-a720-5ea09571489b")
        );
        // input_tokens + cache_read: cache đọc vẫn là token đã trả tiền.
        assert_eq!(out.tokens_in, 16682);
        assert_eq!(out.tokens_out, 4);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn is_error_that_bai_duoc_phan_anh() {
        let (l, d) = log();
        let mut out = AgentOutcome::default();
        let line = r#"{"type":"result","is_error":true,"session_id":"s1","total_cost_usd":0.01}"#;
        handle(&serde_json::from_str(line).unwrap(), &req(), &l, &mut out);
        assert!(!out.ok);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn tool_use_thanh_su_kien_co_ten_va_chi_tiet() {
        let (l, d) = log();
        let mut rx = l.subscribe();
        let mut out = AgentOutcome::default();
        let line = r#"{"type":"assistant","session_id":"s","message":{"content":[
            {"type":"text","text":"đang sửa"},
            {"type":"tool_use","name":"Edit","input":{"file_path":"src/api.rs"}}]}}"#;
        handle(&serde_json::from_str(line).unwrap(), &req(), &l, &mut out);
        let a = rx.try_recv().unwrap();
        assert!(matches!(a.kind, EventKind::AgentText { .. }));
        let b = rx.try_recv().unwrap();
        match b.kind {
            EventKind::AgentTool { name, detail } => {
                assert_eq!(name, "Edit");
                assert_eq!(detail, "src/api.rs");
            }
            _ => panic!("phải là AgentTool"),
        }
        std::fs::remove_dir_all(d).ok();
    }
}
