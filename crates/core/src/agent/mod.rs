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
    /// Bật lên `true` khi orchestrator bị dừng (q trong TUI, Ctrl-C). Adapter
    /// phải giết cả process group của tiến trình agent, không chỉ trả về —
    /// nếu không, huỷ giữa chừng để lại tiến trình mồ côi chạy mãi.
    pub cancel: tokio::sync::watch::Receiver<bool>,
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

/// Đợi tín hiệu huỷ THẬT — không đợi bừa `rx.changed()`.
///
/// Khi không ai giữ `Sender` nữa (ví dụ `agentgraph ask` dùng kênh một lần,
/// không cần huỷ), `changed()` trả `Err` NGAY LẬP TỨC vì sender đã rớt — nếu
/// `select!` coi bất kỳ lần `changed()` hoàn thành nào (kể cả lỗi) là "đã
/// huỷ" thì mọi request không ai giữ sender sẽ bị coi là huỷ tức khắc dù
/// chưa hề có ai gọi `.send(true)`. Đây chính là bug thật đã bắt được khi
/// chạy `ask` với claude thật: agent bị "huỷ" trước khi kịp chạy.
pub(crate) async fn wait_for_cancel(rx: &mut tokio::sync::watch::Receiver<bool>) {
    if rx.changed().await.is_err() {
        // Sẽ không bao giờ có ai gửi huỷ — treo vĩnh viễn để nhánh khác của
        // `select!` (đọc output, timeout) luôn là nhánh thắng.
        std::future::pending::<()>().await;
    }
}

/// Cho tiến trình con một nhóm riêng để sau này giết được cả cây của nó
/// (shell con, dev server agent tự mở...), không chỉ đúng pid trực tiếp.
///
/// Unix: process group mới. Windows: `CREATE_NEW_PROCESS_GROUP` — bản thân cờ
/// này không giết được gì, việc giết cả cây do `taskkill /T` ở dưới đảm nhận,
/// nhưng nó tách tiến trình con khỏi Ctrl-C của console cha.
pub(crate) fn own_process_group(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
}

/// Lệnh chạy một chuỗi shell — dùng cho `verify`. Unix đi qua `sh -c`,
/// Windows qua `cmd /C`; cú pháp lệnh verify vì vậy phụ thuộc nền tảng.
pub(crate) fn shell(script: &str) -> tokio::process::Command {
    #[cfg(unix)]
    {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(script);
        c
    }
    #[cfg(windows)]
    {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(script);
        c
    }
}

/// Giết cả cây tiến trình bắt đầu từ `pid`. Giết mỗi child trực tiếp
/// (`Child::start_kill`) chỉ giết đúng pid đó; tiến trình cháu sống sót và
/// mồ côi mãi mãi. Dùng chung cho claude/codex/fake và verifier.
pub(crate) async fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    let mut c = {
        // `kill -<pgid>` nhắm cả nhóm; `kill` là builtin của sh nên không
        // phụ thuộc binary ngoài.
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(format!("kill -KILL -{pid}"));
        c
    };
    #[cfg(windows)]
    let mut c = {
        // /T giết cả cây con, /F không hỏi.
        let mut c = tokio::process::Command::new("taskkill");
        c.args(["/T", "/F", "/PID", &pid.to_string()]);
        c
    };
    let _ = c
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
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
