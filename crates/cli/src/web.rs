//! Mặt web — vừa là màn hình xem, vừa là nơi khởi động lượt chạy ("studio").
//!
//! Trình duyệt nạp ảnh chụp trạng thái một lần qua `/api/runs/{id}/view`, rồi
//! nghe `/api/runs/{id}/events` (SSE) để cập nhật. Mở tab muộn hay reload đều
//! không mất gì — khác với thiết kế chỉ-stream.
//!
//! SSE đẩy `Patch` **đã fold sẵn**, không đẩy event thô. Client chỉ ghép dữ
//! liệu, không diễn giải event. Để client tự fold thì sẽ có hai bản cùng một
//! quy tắc — một Rust một JS — và chúng sẽ lệch nhau.
//!
//! ## Vì sao có token và kiểm tra Host
//!
//! `POST /api/runs` khởi động agent chạy lệnh shell trong thư mục tuỳ ý. Chỉ
//! lắng nghe `127.0.0.1` là KHÔNG đủ: mọi trang web khác đang mở trong trình
//! duyệt đều gửi được request tới `127.0.0.1`. Hai lớp chặn:
//! - **Token** ngẫu nhiên mỗi lần khởi động, in kèm link ra terminal. Mọi
//!   `/api/*` đều đòi nó. Trang lạ không đọc được terminal nên không có token.
//! - **Host** phải là `127.0.0.1` / `localhost` / `[::1]`. Chặn DNS rebinding:
//!   tên miền của kẻ tấn công trỏ về 127.0.0.1 nhưng header Host vẫn là tên
//!   miền đó.

use agentgraph_core::agent::known_agents;
use agentgraph_core::config::{Limits, Plan};
use agentgraph_core::event::EventLog;
use agentgraph_core::graph::{Isolate, NodeSpec};
use agentgraph_core::ids::{NodeId, RunId};
use agentgraph_core::run::Runner;
use agentgraph_core::view::{Patch, View};
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

/// Alias model claude đã xác nhận bằng `claude --help`. Codex nhận tên model
/// tuỳ ý nên không có danh sách gợi ý — trang web cho gõ tự do.
const CLAUDE_MODELS: &[&str] = &["sonnet", "opus", "fable"];

/// Các giá trị `--permission-mode` claude chấp nhận (theo `claude --help`).
const PERMISSION_MODES: &[&str] = &[
    "acceptEdits",
    "bypassPermissions",
    "plan",
    "auto",
    "manual",
    "dontAsk",
];

/// Một lượt chạy mà server đang giữ: trạng thái đã fold, kênh phát patch, và
/// nút huỷ (nếu lượt chạy còn sống trong tiến trình này).
pub struct RunHandle {
    id: String,
    root: PathBuf,
    events: PathBuf,
    view: Arc<RwLock<View>>,
    /// `None` ở chế độ xem lại: không còn gì phát thêm.
    patches: Option<broadcast::Sender<Patch>>,
    canceller: Option<watch::Sender<bool>>,
}

impl RunHandle {
    /// Lượt chạy live. PHẢI gọi trước khi runner bắt đầu `execute`: view dựng
    /// bằng cách fold event, bỏ lỡ `run_started`/`node_added` là mất sạch graph.
    pub fn live(
        id: String,
        root: PathBuf,
        log: &EventLog,
        canceller: Option<watch::Sender<bool>>,
    ) -> Self {
        let view = Arc::new(RwLock::new(View::default()));
        let (tx, _) = broadcast::channel(4096);
        let v = view.clone();
        let tx2 = tx.clone();
        let mut rx = log.subscribe();
        tokio::spawn(async move {
            loop {
                let ev = match rx.recv().await {
                    Ok(ev) => ev,
                    // Tụt lại sau thì bỏ vài event chứ không dừng fold.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let patch = {
                    let Ok(mut g) = v.write() else { continue };
                    let (node, lines) = g.apply_tracked(&ev);
                    Patch {
                        view: g.meta(),
                        node,
                        lines,
                    }
                };
                let _ = tx2.send(patch);
            }
        });
        Self {
            id,
            root,
            events: log.path().to_path_buf(),
            view,
            patches: Some(tx),
            canceller,
        }
    }

    /// Lượt chạy cũ đọc từ đĩa: view dựng sẵn, không phát gì thêm, không huỷ được.
    pub fn replay(id: String, root: PathBuf, events: PathBuf, view: View) -> Self {
        Self {
            id,
            root,
            events,
            view: Arc::new(RwLock::new(view)),
            patches: None,
            canceller: None,
        }
    }

    fn snapshot(&self) -> View {
        self.view.read().map(|g| g.clone()).unwrap_or_default()
    }
}

#[derive(Clone)]
pub struct App {
    inner: Arc<Inner>,
}

struct Inner {
    token: String,
    /// `false` khi web chỉ để xem một lượt chạy khởi động từ terminal
    /// (`run --web`, `replay --web`): không cho trang web mở lượt chạy mới.
    can_start: bool,
    default_root: PathBuf,
    runs: RwLock<Vec<Arc<RunHandle>>>,
}

impl App {
    pub fn new(can_start: bool, default_root: PathBuf) -> Self {
        // Hai UUID v4 = 244 bit ngẫu nhiên; không cần thêm crate chỉ để sinh token.
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        Self {
            inner: Arc::new(Inner {
                token,
                can_start,
                default_root,
                runs: RwLock::new(Vec::new()),
            }),
        }
    }

    pub fn token(&self) -> &str {
        &self.inner.token
    }

    pub fn add_run(&self, h: RunHandle) {
        if let Ok(mut r) = self.inner.runs.write() {
            r.push(Arc::new(h));
        }
    }

    fn find(&self, id: &str) -> Option<Arc<RunHandle>> {
        self.inner
            .runs
            .read()
            .ok()?
            .iter()
            .find(|h| h.id == id)
            .cloned()
    }
}

pub fn router(app: App) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/meta", get(api_meta))
        .route("/api/check-dir", post(api_check_dir))
        .route("/api/runs", get(api_list_runs).post(api_start_run))
        .route("/api/runs/{id}/view", get(api_view))
        .route("/api/runs/{id}/events", get(api_events))
        .route("/api/runs/{id}/cancel", post(api_cancel))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}

pub async fn serve(app: App, port: u16) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| anyhow::anyhow!("không mở được cổng {port}: {e} — thử --port khác"))?;
    let addr = listener.local_addr()?;
    // Link PHẢI kèm token — mở trang không có token thì mọi API đều bị từ chối.
    println!("web đang chạy: http://{addr}/?token={}", app.token());
    axum::serve(listener, router(app)).await?;
    Ok(())
}

// ── bảo vệ ──────────────────────────────────────────────────────────────────

async fn guard(State(app): State<App>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !host_hop_le(host) {
        return loi(StatusCode::FORBIDDEN, "Host không hợp lệ");
    }
    if req.uri().path().starts_with("/api/") {
        let tu_header = req
            .headers()
            .get("x-agentgraph-token")
            .and_then(|v| v.to_str().ok());
        // EventSource của trình duyệt không đặt được header, nên SSE phải
        // truyền token qua query.
        let tu_query = req
            .uri()
            .query()
            .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")));
        let dung = tu_header
            .or(tu_query)
            .is_some_and(|t| bang_nhau_hang_thoi_gian(t.as_bytes(), app.token().as_bytes()));
        if !dung {
            return loi(
                StatusCode::UNAUTHORIZED,
                "thiếu hoặc sai token — mở đúng link có ?token= được in ra terminal",
            );
        }
    }
    next.run(req).await
}

fn host_hop_le(host: &str) -> bool {
    let ten = match host.strip_prefix('[') {
        // [::1]:7878
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host),
    };
    // Host header không phân biệt hoa thường (RFC 9110 §4.2.3) — `LOCALHOST`
    // hợp lệ y hệt `localhost`. So khớp chữ thường thì mới không chặn nhầm.
    matches!(
        ten.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

/// So sánh không để lộ thời gian: `==` dừng ở byte sai đầu tiên, đo được
/// thời gian là đoán dần được token.
fn bang_nhau_hang_thoi_gian(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Serialize)]
struct LoiJson {
    error: String,
}

fn loi(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(LoiJson { error: msg.into() })).into_response()
}

// ── trang và thông tin chung ────────────────────────────────────────────────

async fn index() -> Html<&'static str> {
    Html(include_str!("../assets/index.html"))
}

#[derive(Serialize)]
struct AgentInfo {
    name: &'static str,
    available: bool,
}

#[derive(Serialize)]
struct Meta {
    can_start: bool,
    default_root: String,
    agents: Vec<AgentInfo>,
    claude_models: &'static [&'static str],
    permission_modes: &'static [&'static str],
}

async fn api_meta(State(app): State<App>) -> Json<Meta> {
    Json(Meta {
        can_start: app.inner.can_start,
        default_root: app.inner.default_root.to_string_lossy().into_owned(),
        // `fake` chỉ để test — không đưa lên giao diện.
        agents: ["claude", "codex"]
            .into_iter()
            .map(|name| AgentInfo {
                name,
                available: crate::which(name).is_some(),
            })
            .collect(),
        claude_models: CLAUDE_MODELS,
        permission_modes: PERMISSION_MODES,
    })
}

// ── kiểm tra thư mục ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CheckDirReq {
    path: String,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
struct DirStatus {
    path: String,
    /// "ok" | "warn" | "error" — "error" nghĩa là không cho chạy.
    level: &'static str,
    message: String,
}

async fn api_check_dir(Json(req): Json<CheckDirReq>) -> Json<DirStatus> {
    Json(kiem_tra_thu_muc(&req.path).await)
}

fn mo_rong_home(p: &str) -> PathBuf {
    let p = p.trim();
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        _ if p == "~" => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        _ => PathBuf::from(p),
    }
}

async fn git_ok(dir: &Path, args: &[&str]) -> bool {
    tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn kiem_tra_thu_muc(raw: &str) -> DirStatus {
    if raw.trim().is_empty() {
        return DirStatus {
            path: String::new(),
            level: "error",
            message: "chưa nhập thư mục".into(),
        };
    }
    let p = mo_rong_home(raw);
    let Ok(abs) = p.canonicalize() else {
        return DirStatus {
            path: p.to_string_lossy().into_owned(),
            level: "error",
            message: "thư mục không tồn tại".into(),
        };
    };
    let path = abs.to_string_lossy().into_owned();
    if !abs.is_dir() {
        return DirStatus {
            path,
            level: "error",
            message: "đây là file, không phải thư mục".into(),
        };
    }
    if !git_ok(&abs, &["rev-parse", "--is-inside-work-tree"]).await {
        return DirStatus {
            path,
            level: "warn",
            message:
                "không phải git repo — agent làm thẳng trong thư mục này, không có worktree riêng"
                    .into(),
        };
    }
    if !git_ok(&abs, &["rev-parse", "--verify", "-q", "HEAD"]).await {
        return DirStatus {
            path,
            level: "error",
            message: "git repo chưa có commit nào — chạy: git add -A && git commit -m init".into(),
        };
    }
    DirStatus {
        path,
        level: "ok",
        message: "git repo — mỗi agent làm việc trên worktree riêng".into(),
    }
}

// ── lượt chạy ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct RunInfo {
    id: String,
    goal: String,
    root: String,
    events: String,
    finished: bool,
    ok: bool,
    total_cost: f64,
    running: usize,
    done: usize,
    failed: usize,
    can_cancel: bool,
}

async fn api_list_runs(State(app): State<App>) -> Json<Vec<RunInfo>> {
    let runs = app.inner.runs.read().map(|r| r.clone()).unwrap_or_default();
    // Mới nhất trước — thứ tự người dùng muốn thấy trong ô chọn.
    let out = runs
        .iter()
        .rev()
        .map(|h| {
            let v = h.snapshot();
            let (running, done, failed, _) = v.counts();
            RunInfo {
                id: h.id.clone(),
                goal: v.goal.clone(),
                root: h.root.to_string_lossy().into_owned(),
                events: h.events.to_string_lossy().into_owned(),
                finished: v.finished,
                ok: v.ok,
                total_cost: v.total_cost,
                running,
                done,
                failed,
                can_cancel: h.canceller.is_some() && !v.finished,
            }
        })
        .collect();
    Json(out)
}

fn mac_dinh_quyen() -> String {
    "acceptEdits".into()
}
fn mac_dinh_ngan_sach() -> f64 {
    20.0
}
fn mac_dinh_song_song() -> usize {
    3
}
fn mac_dinh_timeout() -> u64 {
    30
}

/// Mặc định trùng với `agentgraph run` — hai cửa vào không được có hai bộ
/// mặc định khác nhau.
#[derive(Deserialize, Debug)]
struct StartReq {
    prompt: String,
    agent: String,
    #[serde(default)]
    model: Option<String>,
    dir: String,
    #[serde(default)]
    verify: Option<String>,
    #[serde(default = "mac_dinh_quyen")]
    permission_mode: String,
    #[serde(default = "mac_dinh_ngan_sach")]
    budget: f64,
    #[serde(default = "mac_dinh_song_song")]
    parallel: usize,
    #[serde(default = "mac_dinh_timeout")]
    timeout_min: u64,
}

#[derive(Serialize)]
struct StartResp {
    id: String,
}

fn khong_rong(s: &Option<String>) -> Option<String> {
    s.as_ref()
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
}

/// Kiểm tra mọi thứ không cần đụng đĩa. Tách riêng để test được trọn vẹn.
fn kiem_tra_yeu_cau(r: &StartReq) -> Result<(), String> {
    if r.prompt.trim().is_empty() {
        return Err("prompt đang trống".into());
    }
    if !known_agents().contains(&r.agent.as_str()) {
        return Err(format!("agent '{}' không được hỗ trợ", r.agent));
    }
    if let Some(m) = khong_rong(&r.model) {
        // Model đi thẳng vào argv của agent. Giá trị bắt đầu bằng '-' sẽ bị CLI
        // hiểu thành một flag (ví dụ bật một chế độ quyền) — chặn từ cửa vào.
        if m.starts_with('-') || m.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(format!("tên model không hợp lệ: '{m}'"));
        }
    }
    if !PERMISSION_MODES.contains(&r.permission_mode.as_str()) {
        return Err(format!("chế độ quyền '{}' không hợp lệ", r.permission_mode));
    }
    if !(r.budget.is_finite() && r.budget > 0.0) {
        return Err("ngân sách phải là số dương".into());
    }
    if r.parallel == 0 || r.parallel > 32 {
        return Err("số agent song song phải từ 1 tới 32".into());
    }
    if r.timeout_min == 0 || r.timeout_min > 24 * 60 {
        return Err("timeout phải từ 1 tới 1440 phút".into());
    }
    Ok(())
}

/// Dòng đầu tiên có chữ của prompt, cắt ngắn — làm tiêu đề lượt chạy.
fn muc_tieu_tu_prompt(prompt: &str) -> String {
    let dong = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if dong.chars().count() > 100 {
        format!("{}…", dong.chars().take(100).collect::<String>())
    } else {
        dong.to_string()
    }
}

async fn api_start_run(State(app): State<App>, Json(req): Json<StartReq>) -> Response {
    if !app.inner.can_start {
        return loi(
            StatusCode::FORBIDDEN,
            "web này chỉ để xem — khởi động bằng `agentgraph web` để chạy từ trình duyệt",
        );
    }
    if let Err(e) = kiem_tra_yeu_cau(&req) {
        return loi(StatusCode::BAD_REQUEST, e);
    }
    let dir = kiem_tra_thu_muc(&req.dir).await;
    if dir.level == "error" {
        return loi(StatusCode::BAD_REQUEST, format!("thư mục: {}", dir.message));
    }
    let root = PathBuf::from(&dir.path);

    let plan = Plan {
        goal: muc_tieu_tu_prompt(&req.prompt),
        nodes: vec![NodeSpec {
            id: NodeId::new("chinh").expect("id cố định hợp lệ"),
            title: "agent chính".into(),
            agent: req.agent.clone(),
            task: req.prompt.clone(),
            deps: vec![],
            model: khong_rong(&req.model),
            verify: khong_rong(&req.verify),
            isolate: Isolate::Worktree,
        }],
    };
    let limits = Limits {
        max_parallel: req.parallel,
        max_total_usd: req.budget,
        node_timeout: Duration::from_secs(req.timeout_min * 60),
        permission_mode: req.permission_mode.clone(),
        ..Default::default()
    };

    let run_id = RunId::generate();
    let events = root
        .join(".agentgraph")
        .join("runs")
        .join(run_id.as_str())
        .join("events.jsonl");
    let log = match EventLog::create(&events) {
        Ok(l) => l,
        Err(e) => {
            return loi(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("không tạo được event log: {e}"),
            );
        }
    };
    let id = run_id.as_str().to_string();
    let runner = match Runner::new(&root, limits, log.clone(), run_id).await {
        Ok(r) => r,
        Err(e) => {
            return loi(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("không khởi tạo được lượt chạy: {e}"),
            );
        }
    };
    // Đăng ký với web TRƯỚC khi execute — xem `RunHandle::live`.
    app.add_run(RunHandle::live(
        id.clone(),
        root,
        &log,
        Some(runner.canceller()),
    ));
    tokio::spawn(async move {
        let _ = runner.execute(plan).await;
    });
    (StatusCode::CREATED, Json(StartResp { id })).into_response()
}

async fn api_view(State(app): State<App>, UrlPath(id): UrlPath<String>) -> Response {
    match app.find(&id) {
        Some(h) => Json(h.snapshot()).into_response(),
        None => loi(StatusCode::NOT_FOUND, "không có lượt chạy này"),
    }
}

async fn api_cancel(State(app): State<App>, UrlPath(id): UrlPath<String>) -> Response {
    let Some(h) = app.find(&id) else {
        return loi(StatusCode::NOT_FOUND, "không có lượt chạy này");
    };
    match &h.canceller {
        Some(c) if !h.snapshot().finished => {
            let _ = c.send(true);
            (StatusCode::ACCEPTED, Json(StartResp { id })).into_response()
        }
        _ => loi(
            StatusCode::CONFLICT,
            "lượt chạy đã xong hoặc không huỷ được",
        ),
    }
}

async fn api_events(State(app): State<App>, UrlPath(id): UrlPath<String>) -> Response {
    let Some(h) = app.find(&id) else {
        return loi(StatusCode::NOT_FOUND, "không có lượt chạy này");
    };
    // Không có kênh phát (xem lại lượt cũ) thì trả stream rỗng thay vì lỗi:
    // trình duyệt đã có toàn bộ dữ liệu từ /view rồi.
    let stream: std::pin::Pin<Box<dyn Stream<Item = Result<SseEvent, Infallible>> + Send>> =
        match &h.patches {
            Some(tx) => Box::pin(BroadcastStream::new(tx.subscribe()).filter_map(|r| {
                r.ok().and_then(|p: Patch| {
                    serde_json::to_string(&p)
                        .ok()
                        .map(|j| Ok(SseEvent::default().data(j)))
                })
            })),
            None => Box::pin(tokio_stream::pending()),
        };
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentgraph_core::event::{EventKind, Origin};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ag-web-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tmp_log() -> (EventLog, PathBuf) {
        let d = tmp_dir("log");
        (EventLog::create(d.join("e.jsonl")).unwrap(), d)
    }

    fn git(dir: &Path, args: &[&str]) {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
    }

    fn git_repo_co_commit(tag: &str) -> PathBuf {
        let d = tmp_dir(tag);
        git(&d, &["init", "-q"]);
        git(&d, &["config", "user.email", "a@b"]);
        git(&d, &["config", "user.name", "t"]);
        std::fs::write(d.join("README.md"), "x").unwrap();
        git(&d, &["add", "-A"]);
        git(&d, &["commit", "-qm", "init"]);
        d
    }

    /// Server thật trên cổng ngẫu nhiên. Trả về (cổng, token).
    async fn khoi_dong(can_start: bool) -> (u16, String) {
        let app = App::new(can_start, std::env::temp_dir());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = app.token().to_string();
        tokio::spawn(async move {
            axum::serve(listener, router(app)).await.unwrap();
        });
        (port, token)
    }

    /// HTTP tối giản — không kéo thêm crate client chỉ để test.
    async fn http(
        port: u16,
        method: &str,
        path: &str,
        host: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> (u16, String) {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let body = body.unwrap_or("");
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
        if let Some(t) = token {
            req.push_str(&format!("x-agentgraph-token: {t}\r\n"));
        }
        if !body.is_empty() {
            req.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            ));
        }
        req.push_str("\r\n");
        req.push_str(body);
        s.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.unwrap();
        let text = String::from_utf8_lossy(&buf).into_owned();
        let code = text
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (code, body)
    }

    #[tokio::test]
    async fn live_fold_roi_phat_patch_da_gop_san() {
        let (log, dir) = tmp_log();
        let h = RunHandle::live("r".into(), dir.clone(), &log, None);
        let mut rx = h.patches.as_ref().unwrap().subscribe();
        log.emit(
            None,
            EventKind::RunStarted {
                goal: "mục tiêu".into(),
                run: RunId::generate(),
            },
        );
        assert_eq!(rx.recv().await.unwrap().view.goal, "mục tiêu");
        let n = NodeId::new("a").unwrap();
        log.emit(
            Some(n.clone()),
            EventKind::NodeAdded {
                title: "A".into(),
                agent: "claude".into(),
                deps: vec![],
                by: Origin::Plan,
                model: Some("sonnet".into()),
            },
        );
        let p = rx.recv().await.unwrap();
        assert_eq!(p.view.nodes[0].model.as_deref(), Some("sonnet"));
        log.emit(
            Some(n),
            EventKind::AgentTool {
                name: "Edit".into(),
                detail: "src/api.rs".into(),
            },
        );
        let p = rx.recv().await.unwrap();
        // Client chỉ nối chuỗi này vào, không phải tự hiểu event là gì.
        assert_eq!(p.node.as_deref(), Some("a"));
        assert_eq!(p.lines, vec!["→ Edit src/api.rs".to_string()]);
        assert_eq!(p.view.nodes[0].last, "→ Edit src/api.rs");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn patch_khong_mang_theo_lich_su_log() {
        let (log, dir) = tmp_log();
        let h = RunHandle::live("r".into(), dir.clone(), &log, None);
        let mut rx = h.patches.as_ref().unwrap().subscribe();
        let n = NodeId::new("a").unwrap();
        log.emit(
            Some(n.clone()),
            EventKind::NodeAdded {
                title: "A".into(),
                agent: "fake".into(),
                deps: vec![],
                by: Origin::Plan,
                model: None,
            },
        );
        let _ = rx.recv().await.unwrap();
        log.emit(
            Some(n.clone()),
            EventKind::AgentText {
                text: "DONG_CU_KHONG_DUOC_LAP_LAI".into(),
            },
        );
        let _ = rx.recv().await.unwrap();
        log.emit(
            Some(n),
            EventKind::AgentText {
                text: "dong moi nhat".into(),
            },
        );
        let p = rx.recv().await.unwrap();
        let meta = serde_json::to_string(&p.view).unwrap();
        // `meta` được mang MỘT dòng hoạt động mới nhất (`last`) để vẽ lên node,
        // nhưng không mang lịch sử — nếu không mỗi event đẩy lại toàn bộ log.
        assert!(
            !meta.contains("DONG_CU_KHONG_DUOC_LAP_LAI"),
            "meta mang theo lịch sử log: {meta}"
        );
        assert!(meta.contains("dong moi nhat"));
        assert_eq!(p.lines, vec!["· dong moi nhat".to_string()]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn che_do_xem_lai_khong_co_kenh_phat_va_khong_huy_duoc() {
        let h = RunHandle::replay("r".into(), "/".into(), "/e".into(), View::default());
        assert!(h.patches.is_none(), "replay không được giả vờ còn live");
        assert!(h.canceller.is_none());
    }

    #[test]
    fn host_chi_nhan_dia_chi_may_minh() {
        for ok in [
            "127.0.0.1:7878",
            "localhost:7878",
            "[::1]:7878",
            "localhost",
            // Bug thật: Host không phân biệt hoa thường (RFC 9110 §4.2.3) —
            // so khớp chữ thường sai làm LOCALHOST bị chặn nhầm.
            "LOCALHOST:7878",
            "Localhost",
        ] {
            assert!(host_hop_le(ok), "{ok}");
        }
        // DNS rebinding: tên miền lạ trỏ về 127.0.0.1 nhưng Host vẫn là tên miền đó.
        for xau in [
            "evil.com",
            "evil.com:7878",
            "127.0.0.1.evil.com",
            "",
            "[::2]:1",
        ] {
            assert!(!host_hop_le(xau), "{xau}");
        }
    }

    #[tokio::test]
    async fn api_doi_token_trang_chu_thi_khong() {
        let (port, token) = khoi_dong(true).await;
        let (c, _) = http(port, "GET", "/", "127.0.0.1", None, None).await;
        assert_eq!(c, 200, "trang không chứa dữ liệu nên không cần token");
        let (c, body) = http(port, "GET", "/api/meta", "127.0.0.1", None, None).await;
        assert_eq!(c, 401, "{body}");
        let (c, _) = http(port, "GET", "/api/meta", "127.0.0.1", Some("sai"), None).await;
        assert_eq!(c, 401);
        let (c, body) = http(port, "GET", "/api/meta", "127.0.0.1", Some(&token), None).await;
        assert_eq!(c, 200, "{body}");
        assert!(body.contains("\"can_start\":true"));
        // Token qua query — đường duy nhất EventSource dùng được.
        let q = format!("/api/meta?token={token}");
        let (c, _) = http(port, "GET", &q, "127.0.0.1", None, None).await;
        assert_eq!(c, 200);
    }

    #[tokio::test]
    async fn host_la_bi_tu_choi_ke_ca_khi_co_token() {
        let (port, token) = khoi_dong(true).await;
        let (c, _) = http(port, "GET", "/api/meta", "evil.com", Some(&token), None).await;
        assert_eq!(c, 403);
        let (c, _) = http(port, "GET", "/", "evil.com:80", None, None).await;
        assert_eq!(c, 403);
    }

    #[tokio::test]
    async fn kiem_tra_thu_muc_phan_biet_cac_truong_hop() {
        let khong_git = tmp_dir("nogit");
        assert_eq!(
            kiem_tra_thu_muc(khong_git.to_str().unwrap()).await.level,
            "warn"
        );
        let chua_commit = tmp_dir("nocommit");
        git(&chua_commit, &["init", "-q"]);
        let s = kiem_tra_thu_muc(chua_commit.to_str().unwrap()).await;
        assert_eq!(s.level, "error");
        assert!(s.message.contains("commit"), "{}", s.message);
        let tot = git_repo_co_commit("ok");
        assert_eq!(kiem_tra_thu_muc(tot.to_str().unwrap()).await.level, "ok");
        assert_eq!(kiem_tra_thu_muc("/khong/he/ton/tai").await.level, "error");
        assert_eq!(kiem_tra_thu_muc("   ").await.level, "error");
        for d in [khong_git, chua_commit, tot] {
            std::fs::remove_dir_all(d).ok();
        }
    }

    fn req_hop_le() -> StartReq {
        serde_json::from_str(r#"{"prompt":"x","agent":"claude","dir":"/tmp"}"#).unwrap()
    }

    #[test]
    fn mac_dinh_trung_voi_lenh_run() {
        let r = req_hop_le();
        assert_eq!(r.permission_mode, "acceptEdits");
        assert_eq!(r.budget, 20.0);
        assert_eq!(r.parallel, 3);
        assert_eq!(r.timeout_min, 30);
        assert!(kiem_tra_yeu_cau(&r).is_ok());
    }

    #[test]
    fn yeu_cau_xau_bi_chan_voi_ly_do_ro() {
        let mut r = req_hop_le();
        r.prompt = "   ".into();
        assert!(kiem_tra_yeu_cau(&r).unwrap_err().contains("prompt"));

        let mut r = req_hop_le();
        r.agent = "skynet".into();
        assert!(kiem_tra_yeu_cau(&r).is_err());

        // Model đi vào argv — giá trị dạng flag không được lọt qua.
        let mut r = req_hop_le();
        r.model = Some("--dangerously-skip-permissions".into());
        assert!(kiem_tra_yeu_cau(&r).unwrap_err().contains("model"));
        let mut r = req_hop_le();
        r.model = Some("sonnet --x".into());
        assert!(kiem_tra_yeu_cau(&r).is_err());

        let mut r = req_hop_le();
        r.permission_mode = "yolo".into();
        assert!(kiem_tra_yeu_cau(&r).is_err());

        let mut r = req_hop_le();
        r.budget = f64::NAN;
        assert!(kiem_tra_yeu_cau(&r).is_err());
        let mut r = req_hop_le();
        r.parallel = 0;
        assert!(kiem_tra_yeu_cau(&r).is_err());
    }

    #[test]
    fn muc_tieu_lay_dong_dau_co_chu() {
        assert_eq!(muc_tieu_tu_prompt("\n\n  Sửa test\nchi tiết"), "Sửa test");
        let dai = "a".repeat(300);
        assert_eq!(muc_tieu_tu_prompt(&dai).chars().count(), 101);
    }

    /// Chờ tới khi lượt chạy xong, trả về body của /view.
    async fn cho_xong(port: u16, token: &str, id: &str) -> String {
        let path = format!("/api/runs/{id}/view");
        for _ in 0..100 {
            let (_, body) = http(port, "GET", &path, "127.0.0.1", Some(token), None).await;
            if body.contains("\"finished\":true") {
                return body;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("lượt chạy {id} không xong trong 10 giây");
    }

    fn lay_id(body: &str) -> String {
        body.split("\"id\":\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap_or_default()
            .to_string()
    }

    #[tokio::test]
    async fn chay_tu_trinh_duyet_tu_dau_toi_cuoi() {
        let (port, token) = khoi_dong(true).await;
        let repo = git_repo_co_commit("start");
        let body = serde_json::json!({
            "prompt": "Việc chính\nchi tiết",
            "agent": "fake",
            "model": "sonnet",
            "dir": repo,
            "verify": "true",
        })
        .to_string();
        let (c, resp) = http(
            port,
            "POST",
            "/api/runs",
            "127.0.0.1",
            Some(&token),
            Some(&body),
        )
        .await;
        assert_eq!(c, 201, "{resp}");
        let id = lay_id(&resp);

        let view = cho_xong(port, &token, &id).await;
        assert!(view.contains("\"goal\":\"Việc chính\""), "{view}");
        assert!(view.contains("\"state\":\"done\""), "{view}");
        assert!(
            view.contains("\"model\":\"sonnet\""),
            "model phải hiện trên node: {view}"
        );
        assert!(
            view.contains("\"workspace\":\""),
            "phải biết thư mục agent làm việc: {view}"
        );

        let (_, list) = http(port, "GET", "/api/runs", "127.0.0.1", Some(&token), None).await;
        assert!(list.contains(&id));
        assert!(list.contains("\"ok\":true"), "{list}");
        // Event log nằm trong chính project đã chọn.
        assert!(
            repo.join(".agentgraph/runs")
                .join(&id)
                .join("events.jsonl")
                .exists()
        );
        std::fs::remove_dir_all(repo).ok();
    }

    #[tokio::test]
    async fn chay_bi_tu_choi_khi_web_chi_de_xem() {
        let (port, token) = khoi_dong(false).await;
        let repo = git_repo_co_commit("readonly");
        let body = serde_json::json!({"prompt": "x", "agent": "fake", "dir": repo}).to_string();
        let (c, _) = http(
            port,
            "POST",
            "/api/runs",
            "127.0.0.1",
            Some(&token),
            Some(&body),
        )
        .await;
        assert_eq!(c, 403);
        std::fs::remove_dir_all(repo).ok();
    }

    #[tokio::test]
    async fn repo_chua_commit_bi_chan_truoc_khi_chay() {
        let (port, token) = khoi_dong(true).await;
        let d = tmp_dir("start-nocommit");
        git(&d, &["init", "-q"]);
        let body = serde_json::json!({"prompt": "x", "agent": "fake", "dir": d}).to_string();
        let (c, resp) = http(
            port,
            "POST",
            "/api/runs",
            "127.0.0.1",
            Some(&token),
            Some(&body),
        )
        .await;
        assert_eq!(c, 400);
        assert!(resp.contains("commit"), "{resp}");
        std::fs::remove_dir_all(d).ok();
    }

    #[tokio::test]
    async fn dung_luot_chay_tu_trinh_duyet() {
        let (port, token) = khoi_dong(true).await;
        let repo = git_repo_co_commit("cancel");
        let body =
            serde_json::json!({"prompt": "SLEEP:60", "agent": "fake", "dir": repo}).to_string();
        let (_, resp) = http(
            port,
            "POST",
            "/api/runs",
            "127.0.0.1",
            Some(&token),
            Some(&body),
        )
        .await;
        let id = lay_id(&resp);
        // Đợi agent thật sự đang chạy rồi mới huỷ.
        let path = format!("/api/runs/{id}/view");
        for _ in 0..50 {
            let (_, v) = http(port, "GET", &path, "127.0.0.1", Some(&token), None).await;
            if v.contains("\"state\":\"running\"") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let cancel = format!("/api/runs/{id}/cancel");
        let t = std::time::Instant::now();
        let (c, _) = http(port, "POST", &cancel, "127.0.0.1", Some(&token), None).await;
        assert_eq!(c, 202);
        let view = cho_xong(port, &token, &id).await;
        assert!(t.elapsed() < Duration::from_secs(10), "huỷ phải dừng ngay");
        assert!(view.contains("\"state\":\"failed\""), "{view}");
        // Huỷ lần hai khi đã xong: báo rõ thay vì giả vờ thành công.
        let (c, _) = http(port, "POST", &cancel, "127.0.0.1", Some(&token), None).await;
        assert_eq!(c, 409);
        std::fs::remove_dir_all(repo).ok();
    }
}
