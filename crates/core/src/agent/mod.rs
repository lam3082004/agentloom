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
/// Khi không ai giữ `Sender` nữa (ví dụ `agentloom ask` dùng kênh một lần,
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

/// Mọi hậu duệ (con, cháu, chắt...) của `root`, lần theo quan hệ cha–con.
///
/// Không dựa vào process group: sandbox của codex chạy lệnh trong session MỚI
/// (setsid), nên nhóm tiến trình của agent không chứa chúng. Quan hệ cha–con
/// thì vẫn còn nguyên — miễn là cha chưa chết.
#[cfg(unix)]
async fn hau_due(root: u32) -> Vec<u32> {
    let mut tat_ca: Vec<u32> = Vec::new();
    let mut can_tham = vec![root];
    while let Some(p) = can_tham.pop() {
        // Trần an toàn: không để một cây bất thường làm vòng này chạy mãi.
        if tat_ca.len() > 4096 {
            break;
        }
        let Ok(out) = tokio::process::Command::new("pgrep")
            .args(["-P", &p.to_string()])
            .output()
            .await
        else {
            break; // không có pgrep: chỉ còn trông vào kill theo nhóm
        };
        for dong in String::from_utf8_lossy(&out.stdout).lines() {
            if let Ok(con) = dong.trim().parse::<u32>() {
                if con != root && !tat_ca.contains(&con) {
                    tat_ca.push(con);
                    can_tham.push(con);
                }
            }
        }
    }
    tat_ca
}

#[cfg(unix)]
async fn gui_tin_hieu(sig: &str, targets: &[String]) {
    if targets.is_empty() {
        return;
    }
    // `kill` builtin của sh xử lý từng đích; một đích đã chết không chặn các đích khác.
    let _ = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(format!("kill -{sig} {} 2>/dev/null", targets.join(" ")))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
}

/// Giết cả cây tiến trình bắt đầu từ `pid`. Giết mỗi child trực tiếp
/// (`Child::start_kill`) chỉ giết đúng pid đó; tiến trình cháu sống sót và
/// mồ côi mãi mãi. Dùng chung cho claude/codex/fake và verifier.
///
/// Gọi hàm này TRƯỚC khi giết chính `pid`: cha chết là con bị chuyển về init,
/// dấu vết cha–con mất và không còn lần ra để giết.
pub(crate) async fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    {
        // 1. Đóng băng cả cây: tiến trình bị STOP không đẻ thêm con được, nên
        //    lần gom thứ hai là trọn vẹn.
        let mut cay = hau_due(pid).await;
        let dich: Vec<String> = std::iter::once(format!("-{pid}"))
            .chain(std::iter::once(pid.to_string()))
            .chain(cay.iter().map(u32::to_string))
            .collect();
        gui_tin_hieu("STOP", &dich).await;
        // 2. Gom lại trên cây đã đứng yên.
        for p in hau_due(pid).await {
            if !cay.contains(&p) {
                cay.push(p);
            }
        }
        // 3. Giết nhóm của agent, rồi từng hậu duệ — kể cả những cái đã tách
        //    session riêng mà `kill -<pgid>` không với tới.
        let dich: Vec<String> = std::iter::once(format!("-{pid}"))
            .chain(std::iter::once(pid.to_string()))
            .chain(cay.iter().map(u32::to_string))
            .collect();
        gui_tin_hieu("KILL", &dich).await;
    }
    #[cfg(windows)]
    {
        // /T giết cả cây con theo quan hệ cha–con, /F không hỏi.
        let _ = tokio::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::io::AsyncBufReadExt;

    fn con_song(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Bug thật với codex: sandbox của nó chạy lệnh trong session MỚI (setsid),
    /// nên `kill -<pgid>` không với tới — huỷ xong vẫn còn `sleep 45` sống
    /// mồ côi. Giết phải đi theo cây cha–con, không theo process group.
    #[tokio::test]
    async fn giet_ca_tien_trinh_chau_da_tach_session() {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg("setsid sleep 60 & echo $!; wait")
            .stdout(std::process::Stdio::piped());
        own_process_group(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let pid = child.id().unwrap();
        let mut out = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let chau: u32 = out
            .next_line()
            .await
            .unwrap()
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(con_song(chau), "tiền đề: tiến trình cháu phải đang sống");

        kill_process_group(pid).await;
        let _ = child.wait().await;
        // Tiến trình bị giết cần một chút để hệ điều hành dọn.
        for _ in 0..20 {
            if !con_song(chau) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let con = con_song(chau);
        if con {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &chau.to_string()])
                .status();
        }
        assert!(!con, "tiến trình cháu đã setsid vẫn sống sau khi huỷ");
    }
}
