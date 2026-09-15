//! Adapter cho agent bên ngoài.
//!
//! Hệ này KHÔNG tự gọi model. Mỗi node là một tiến trình agent có sẵn
//! (claude code, codex) chạy headless, tự nó đã có harness riêng. Việc của
//! adapter chỉ là: dựng lệnh, đọc NDJSON, dịch sang `Event`, và trả về kết
//! quả đủ để lập lịch tiếp — nhất là `session` để lần sau `--resume`.

pub mod claude;
pub mod codex;
pub mod fake;

use crate::event::EventLog;
use crate::ids::NodeId;
use async_trait::async_trait;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub node: NodeId,
    pub task: String,
    /// Chỉ dẫn của hệ điều phối. Truyền qua kênh operator (system prompt),
    /// KHÔNG nhét vào thân task — agent có ý thức bảo mật sẽ coi lệnh lạ
    /// trong task là prompt injection và từ chối, rất đúng.
    pub protocol: String,
    pub cwd: PathBuf,
    /// Có thì resume phiên cũ; không thì mở phiên mới.
    pub session: Option<String>,
    pub model: Option<String>,
    /// Chỉ dùng cho claude: acceptEdits | bypassPermissions | plan | ...
    pub permission_mode: String,
    /// Timeout cứng cho một node.
    pub timeout: std::time::Duration,
}

#[derive(Debug, Clone, Default)]
pub struct AgentOutcome {
    pub session: Option<String>,
    pub ok: bool,
    pub summary: String,
    pub cost_usd: f64,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

#[async_trait]
pub trait AgentAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    /// Lệnh cần có trên PATH, để `doctor` kiểm tra trước khi chạy.
    fn binary(&self) -> &'static str;
    async fn run(&self, req: AgentRequest, log: &EventLog) -> anyhow::Result<AgentOutcome>;
}

pub fn adapter_for(name: &str) -> Option<Box<dyn AgentAdapter>> {
    match name {
        "claude" => Some(Box::new(claude::ClaudeAdapter)),
        "codex" => Some(Box::new(codex::CodexAdapter)),
        "fake" => Some(Box::new(fake::FakeAdapter::default())),
        _ => None,
    }
}

pub fn known_agents() -> &'static [&'static str] {
    &["claude", "codex", "fake"]
}
