mod tui;
mod web;

use agentloom_core::agent::{AgentRequest, adapter_for, known_agents};
use agentloom_core::config::{Limits, Plan};
use agentloom_core::event::EventLog;
use agentloom_core::graph::{Isolate, NodeSpec};
use agentloom_core::ids::NodeId;
use agentloom_core::run::Runner;
use anyhow::Context;
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "agentloom",
    version,
    about = "Điều phối nhiều coding agent theo graph động"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Chạy một plan (hoặc một mục tiêu một-node) và xem live.
    Run {
        /// File plan TOML. Không có thì dùng --goal.
        plan: Option<PathBuf>,
        /// Mục tiêu cho plan một node, khi không có file plan.
        #[arg(long)]
        goal: Option<String>,
        /// Agent cho plan một node.
        #[arg(long, default_value = "claude")]
        agent: String,
        /// Thư mục gốc của project.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value_t = 3)]
        parallel: usize,
        /// Trần chi phí cả lượt chạy, USD.
        #[arg(long, default_value_t = 20.0)]
        budget: f64,
        /// Timeout mỗi node, phút.
        #[arg(long, default_value_t = 30)]
        timeout_min: u64,
        #[arg(long, default_value = "acceptEdits")]
        permission_mode: String,
        /// Bỏ TUI, chỉ in dòng — hợp để pipe hoặc chạy trong CI.
        #[arg(long)]
        plain: bool,
        /// Mở giao diện web thay vì TUI.
        #[arg(long)]
        web: bool,
        #[arg(long, default_value_t = 7878)]
        port: u16,
    },
    /// Kiểm tra môi trường trước khi chạy thật.
    Doctor,
    /// Xem lại một lượt chạy cũ từ event log.
    Replay {
        events: PathBuf,
        #[arg(long)]
        plain: bool,
        /// Xem lại trong trình duyệt thay vì TUI.
        #[arg(long)]
        web: bool,
        #[arg(long, default_value_t = 7878)]
        port: u16,
    },
    /// Hỏi lại agent của một node cũ — resume đúng session trong worktree cũ
    /// của nó, không chạy lại từ đầu.
    Ask {
        /// Event log của lượt chạy chứa node cần hỏi.
        events: PathBuf,
        /// Id node muốn resume.
        node: String,
        /// Câu hỏi gửi cho agent.
        question: String,
        /// In event JSONL của lượt resume thay vì chỉ câu trả lời cuối.
        #[arg(long)]
        plain: bool,
    },
    /// Liệt kê các lượt chạy trong `.agentloom/runs/`, mới nhất trước.
    Runs {
        /// Thư mục gốc chứa `.agentloom/runs/` — mặc định thư mục hiện tại.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Mở giao diện web: gõ prompt, chọn agent, model, thư mục rồi bấm chạy —
    /// xem graph các agent hoạt động trực tiếp.
    Web {
        #[arg(long, default_value_t = 7878)]
        port: u16,
        /// Thư mục điền sẵn trong ô "thư mục project".
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Doctor => doctor().await,
        Cmd::Replay {
            events,
            plain,
            web,
            port,
        } => replay(events, plain, web, port).await,
        Cmd::Ask {
            events,
            node,
            question,
            plain,
        } => ask(events, node, question, plain).await,
        Cmd::Runs { root } => runs(root).await,
        Cmd::Web { port, root } => {
            let root = root.canonicalize().unwrap_or(root);
            web::serve(web::App::new(true, root), port).await
        }
        Cmd::Run {
            plan,
            goal,
            agent,
            root,
            parallel,
            budget,
            timeout_min,
            permission_mode,
            plain,
            web,
            port,
        } => {
            // Trần vô nghĩa phải chết ngay ở đây: 0 node song song thì không
            // có gì chạy được, 0 phút thì node nào cũng hết giờ tức khắc.
            if parallel == 0 {
                anyhow::bail!("--parallel phải ít nhất là 1");
            }
            if timeout_min == 0 {
                anyhow::bail!("--timeout-min phải ít nhất là 1");
            }
            if budget.is_nan() || budget <= 0.0 {
                anyhow::bail!("--budget phải lớn hơn 0 (đang là {budget})");
            }
            let plan = match (plan, goal) {
                (Some(p), _) => {
                    let s = std::fs::read_to_string(&p)
                        .with_context(|| format!("không đọc được plan {}", p.display()))?;
                    let plan = Plan::from_toml(&s)?;
                    plan.validate()
                        .with_context(|| format!("plan {} không hợp lệ", p.display()))?;
                    plan
                }
                (None, Some(g)) => Plan {
                    goal: g.clone(),
                    nodes: vec![NodeSpec {
                        id: NodeId::new("root").unwrap(),
                        title: "root".into(),
                        agent,
                        task: g,
                        deps: vec![],
                        model: None,
                        verify: None,
                        isolate: Isolate::Worktree,
                    }],
                },
                (None, None) => anyhow::bail!("cần một file plan hoặc --goal"),
            };
            let limits = Limits {
                max_parallel: parallel,
                max_total_usd: budget,
                node_timeout: Duration::from_secs(timeout_min * 60),
                permission_mode,
                ..Default::default()
            };
            run(plan, root, limits, plain, web, port).await
        }
    }
}

async fn run(
    plan: Plan,
    root: PathBuf,
    limits: Limits,
    plain: bool,
    web: bool,
    port: u16,
) -> anyhow::Result<()> {
    let root = root.canonicalize().unwrap_or(root);
    let run_id = agentloom_core::ids::RunId::generate();
    let dir = root.join(".agentloom").join("runs").join(run_id.as_str());
    let events = dir.join("events.jsonl");
    let log = EventLog::create(&events)?;
    let mut rx = log.subscribe();

    let run_id_str = run_id.as_str().to_string();
    let runner = Runner::new(&root, limits, log.clone(), run_id).await?;
    let canceller = runner.canceller();
    // Mặt web phải đăng ký *trước* khi runner chạy: nó dựng trạng thái bằng
    // cách fold event, nên bỏ lỡ run_started/node_added là mất sạch graph.
    let web_state = web.then(|| {
        // Web khởi động từ terminal chỉ để XEM lượt chạy này, không mở lượt mới.
        let app = web::App::new(false, root.clone());
        app.add_run(web::RunHandle::live(
            run_id_str.clone(),
            root.clone(),
            &log,
            Some(canceller.clone()),
        ));
        app
    });
    let mut handle = tokio::spawn(async move { runner.execute(plan).await });

    // Ctrl-C ở `--plain`/`--web` không còn tự giết được tiến trình agent con:
    // adapter giờ spawn vào process group RIÊNG (để giết được cả tiến trình
    // cháu), nên terminal không còn tự chuyển SIGINT tới chúng như trước.
    // TUI xử lý Ctrl-C như một phím (raw mode chặn SIGINT thật), nhưng vẫn
    // bắt ở đây phòng khi không phải terminal tương tác.
    {
        let c = canceller.clone();
        let mut cancel_rx = log.subscribe();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\nđang huỷ — giết tiến trình agent...");
                let _ = c.send(true);
                // Chờ `run_finished` THẬT thay vì ngủ cố định 3 giây: khi
                // không còn gì đang chạy, run_finished tới gần như ngay —
                // ngủ cố định bắt người dùng chờ vô ích. Khi kill + verifier
                // + ghi log chậm hơn dự kiến, ngủ cố định lại thoát TRƯỚC
                // khi run_finished kịp ghi, mất bằng chứng lượt chạy đã bị
                // huỷ. Vẫn giữ trần 30s để không treo mãi nếu event log kẹt.
                let cho_xong = async {
                    loop {
                        match cancel_rx.recv().await {
                            Ok(ev) => {
                                if matches!(
                                    ev.kind,
                                    agentloom_core::event::EventKind::RunFinished { .. }
                                ) {
                                    break;
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                };
                tokio::select! {
                    _ = cho_xong => {}
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {
                        eprintln!("huỷ quá 30s — thoát cứng, run_finished có thể chưa kịp ghi");
                    }
                }
                std::process::exit(130);
            }
        });
    }

    if let Some(state) = web_state {
        // Web chạy tới khi người dùng Ctrl-C: graph xong rồi vẫn xem lại được.
        // Server nằm trong task riêng vì `select!` huỷ nhánh thua — để nguyên
        // trong select thì graph xong là server chết, trái hẳn lời nhắn.
        println!("event log: {}", events.display());
        let mut server = tokio::spawn(async move { web::serve(state, port).await });
        tokio::select! {
            r = &mut server => r??,
            r = &mut handle => {
                let s = r??;
                println!(
                    "\n{} · {} xong · {} hỏng · {} bỏ qua · ${:.2}",
                    if s.ok { "OK" } else { "CÓ LỖI" },
                    s.done, s.failed, s.skipped, s.total_cost_usd
                );
                println!("graph đã xong — web vẫn mở, Ctrl-C để thoát");
                server.await??;
            }
        }
        return Ok(());
    }

    if plain {
        // Chế độ dòng: mọi event in ra ngay, pipe/grep được.
        // Dừng ở run_finished: EventLog còn sống trong scope này nên kênh
        // broadcast không bao giờ tự đóng.
        let mut out = std::io::stdout();
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    // `println!` panic khi đầu kia đóng pipe (`| head`), mà
                    // chế độ này sinh ra chính là để pipe.
                    if writeln!(out, "{}", serde_json::to_string(&ev)?).is_err() {
                        break;
                    }
                    if matches!(
                        ev.kind,
                        agentloom_core::event::EventKind::RunFinished { .. }
                    ) {
                        break;
                    }
                }
                // Tụt lại sau vì quá nhiều event không phải lý do để bỏ in nốt.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    } else {
        let mut term = ratatui::init();
        let mut ui = tui::Ui::new();
        loop {
            // Vét hết event đang chờ rồi mới vẽ một lần.
            loop {
                match rx.try_recv() {
                    Ok(ev) => ui.view.apply(&ev),
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
                }
            }
            term.draw(|f| ui.draw(f))?;
            if !ui.handle_input(Duration::from_millis(80))? {
                // q/Esc/Ctrl-C: huỷ agent đang chạy trước khi thoát vòng vẽ.
                // Vô hại nếu graph đã xong — không còn gì để giết.
                let _ = canceller.send(true);
                break;
            }
            if ui.view.finished && handle.is_finished() {
                // Để người dùng đọc kết quả; thoát bằng q.
            }
        }
        ratatui::restore();
    }

    let summary = handle.await??;
    let _ = writeln!(
        std::io::stdout(),
        "\n{} · {} xong · {} hỏng · {} bỏ qua · ${:.2}\nevent log: {}",
        if summary.ok { "OK" } else { "CÓ LỖI" },
        summary.done,
        summary.failed,
        summary.skipped,
        summary.total_cost_usd,
        summary.events_path.display()
    );
    if !summary.ok {
        std::process::exit(1);
    }
    Ok(())
}

async fn replay(events: PathBuf, plain: bool, web: bool, port: u16) -> anyhow::Result<()> {
    let evs = EventLog::replay(&events)
        .with_context(|| format!("không đọc được event log {}", events.display()))?;
    let mut ui = tui::Ui::new();
    for e in &evs {
        ui.view.apply(e);
    }
    if web {
        let run_dir = events.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let id = run_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "xem-lai".into());
        // .agentloom/runs/<id>/events.jsonl → gốc project nằm trên ba cấp.
        let root = run_dir
            .ancestors()
            .nth(3)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| run_dir.clone());
        let app = web::App::new(false, root.clone());
        app.add_run(web::RunHandle::replay(id, root, events.clone(), ui.view));
        return web::serve(app, port).await;
    }
    if plain {
        // Như chế độ dòng của `run`: đầu kia đóng pipe thì dừng, không panic.
        let mut out = std::io::stdout();
        for (id, n) in ui
            .view
            .order
            .iter()
            .filter_map(|i| Some((i, ui.view.nodes.get(i)?)))
        {
            if writeln!(
                out,
                "{:<20} {:<8} ${:.2}  {}",
                id, n.state, n.cost, n.summary
            )
            .is_err()
            {
                return Ok(());
            }
        }
        let _ = writeln!(out, "tổng ${:.2} · {} event", ui.view.total_cost, evs.len());
        return Ok(());
    }
    let mut term = ratatui::init();
    loop {
        term.draw(|f| ui.draw(f))?;
        if !ui.handle_input(Duration::from_millis(120))? {
            break;
        }
    }
    ratatui::restore();
    Ok(())
}

/// Hỏi lại agent của một node cũ: đọc log để tìm agent + session + worktree
/// của nó, rồi resume đúng phiên đó — không chạy lại từ đầu, không tạo
/// worktree mới. Dùng lại `AgentAdapter` y hệt lúc chạy thật, chỉ khác
/// `protocol` rỗng: đây là câu hỏi của người, không phải chỉ dẫn harness.
async fn ask(events: PathBuf, node: String, question: String, plain: bool) -> anyhow::Result<()> {
    let evs = EventLog::replay(&events)
        .with_context(|| format!("không đọc được event log {}", events.display()))?;
    let node_id =
        NodeId::new(&node).map_err(|e| anyhow::anyhow!("node id '{node}' không hợp lệ: {e}"))?;

    let mut agent = None;
    let mut cwd = None;
    let mut session = None;
    let mut found = false;
    for e in &evs {
        if e.node.as_ref() != Some(&node_id) {
            continue;
        }
        found = true;
        match &e.kind {
            agentloom_core::event::EventKind::NodeAdded { agent: a, .. } => {
                agent = Some(a.clone());
            }
            // Node có thể được resume nhiều lần với worktree khác nhau chỉ
            // khi bị spawn lại — trong một lượt chạy, dòng cuối là đúng.
            agentloom_core::event::EventKind::Workspace { path, .. } => {
                cwd = Some(PathBuf::from(path));
            }
            agentloom_core::event::EventKind::NodeFinished { session: s, .. } if s.is_some() => {
                session = s.clone();
            }
            _ => {}
        }
    }

    if !found {
        anyhow::bail!("node '{node}' không tồn tại trong {}", events.display());
    }
    let agent =
        agent.ok_or_else(|| anyhow::anyhow!("node '{node}' không có bản ghi agent trong log"))?;
    let session = session.ok_or_else(|| {
        anyhow::anyhow!(
            "node '{node}' không có session — chưa chạy xong lần nào nên không resume được"
        )
    })?;
    let cwd = cwd.ok_or_else(|| anyhow::anyhow!("node '{node}' không có workspace trong log"))?;
    if !cwd.exists() {
        anyhow::bail!("worktree của node '{node}' đã bị xoá: {}", cwd.display());
    }
    let Some(adapter) = adapter_for(&agent) else {
        anyhow::bail!("agent '{agent}' không có adapter");
    };

    // Log riêng cho lượt hỏi lại, cạnh log gốc — không trộn vào events.jsonl
    // của lượt chạy cũ, nhưng vẫn theo đúng nguyên tắc "sự thật nằm ở log".
    let ask_dir = events
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let ask_path = ask_dir.join(format!(
        "ask-{node}-{}.jsonl",
        agentloom_core::ids::RunId::generate()
    ));
    let log = EventLog::create(&ask_path)?;
    let mut rx = log.subscribe();

    let req = AgentRequest {
        node: node_id,
        task: question,
        protocol: String::new(),
        cwd,
        session: Some(session),
        model: None,
        permission_mode: "acceptEdits".into(),
        timeout: Duration::from_secs(30 * 60),
        cancel: tokio::sync::watch::channel(false).1,
    };

    let out = adapter.run(req, &log).await?;
    if plain {
        // `emit` ghi đĩa rồi mới broadcast, và toàn bộ event đã phát ra
        // trước khi `run` trả về — không cần chạy song song với adapter,
        // channel đã có sẵn hết để đọc lại.
        while let Ok(ev) = rx.try_recv() {
            if writeln!(std::io::stdout(), "{}", serde_json::to_string(&ev)?).is_err() {
                break;
            }
        }
        println!();
    }
    println!("{}", out.summary);
    Ok(())
}

/// Liệt kê `.agentloom/runs/*/events.jsonl`, mới nhất trước. Dùng lại
/// `View::apply` để fold — id run có tiền tố `%Y%m%d-%H%M%S` (xem
/// `RunId::generate`) nên sắp xếp chuỗi cũng chính là sắp theo thời gian,
/// không cần phân tích lại timestamp.
async fn runs(root: PathBuf) -> anyhow::Result<()> {
    let dir = root.join(".agentloom").join("runs");
    let mut ids: Vec<String> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect(),
        Err(_) => {
            println!("chưa có lượt chạy nào trong {}", dir.display());
            return Ok(());
        }
    };
    if ids.is_empty() {
        println!("chưa có lượt chạy nào trong {}", dir.display());
        return Ok(());
    }
    ids.sort();
    ids.reverse();

    for id in ids {
        let events = dir.join(&id).join("events.jsonl");
        // Log hỏng hoặc lượt chạy chưa xong (không có run_finished, ví dụ
        // tiến trình bị kill -9) không được làm chết cả lệnh — hiện là dở
        // dang chứ không phải lỗi của `runs`.
        let evs = match agentloom_core::event::EventLog::replay(&events) {
            Ok(evs) => evs,
            Err(_) => {
                println!("{id:<24} (không đọc được log)");
                continue;
            }
        };
        let mut v = agentloom_core::view::View::default();
        for e in &evs {
            v.apply(e);
        }
        // Đếm trên trạng thái node đã fold sẵn trong View — không đọc lại
        // event thô lần hai, đúng nguyên tắc "một phép fold, ba mặt" áp dụng
        // luôn cho mặt thứ tư này.
        let (mut done, mut failed, mut skipped, mut pending, mut running) = (0, 0, 0, 0, 0);
        for n in v.nodes.values() {
            match n.state.as_str() {
                "done" => done += 1,
                "failed" => failed += 1,
                "skipped" => skipped += 1,
                "running" => running += 1,
                // blocked/ready: huỷ giữa chừng hay chạm trần ngân sách để lại
                // node chưa từng chạy — bỏ qua chúng thì tổng nhỏ hơn plan thật.
                _ => pending += 1,
            }
        }
        // Chỉ hiện khi có: lượt đã xong thì không thể còn node đang chạy, in
        // "0 đang chạy" ở mọi dòng chỉ thêm nhiễu.
        let dang_chay = if running > 0 {
            format!(" · {running} đang chạy")
        } else {
            String::new()
        };
        let trang_thai = if !v.finished {
            "DỞ DANG"
        } else if v.ok {
            "OK"
        } else {
            "CÓ LỖI"
        };
        println!(
            "{id:<24} {trang_thai:<8} {done} xong · {failed} hỏng · {skipped} bỏ qua · {pending} chưa chạy{dang_chay} · ${:.2} · {}",
            v.total_cost, v.goal
        );
    }
    Ok(())
}

async fn doctor() -> anyhow::Result<()> {
    println!("agentloom doctor\n");
    let mut missing = 0;
    for name in known_agents() {
        let Some(ad) = adapter_for(name) else {
            continue;
        };
        let bin = ad.binary();
        let found = which(bin);
        match &found {
            Some(p) => println!("  ✓ {name:<8} {bin} → {p}"),
            None => {
                missing += 1;
                println!("  ✕ {name:<8} {bin} không có trên PATH");
            }
        }
    }
    let git = which("git");
    println!("\n  {} git", if git.is_some() { "✓" } else { "✕" });
    let cwd = std::env::current_dir()?;
    let is_repo = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&cwd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    // Là repo chưa đủ: worktree tách nhánh từ HEAD, repo vừa `git init` chưa
    // có HEAD nên mọi node isolate=worktree sẽ hỏng ngay lượt đầu.
    let has_commit = is_repo
        && std::process::Command::new("git")
            .args(["rev-parse", "--verify", "-q", "HEAD"])
            .current_dir(&cwd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
    let (mark, msg) = match (is_repo, has_commit) {
        (true, true) => ("✓", "là git repo — cô lập worktree bật"),
        (true, false) => (
            "!",
            "là git repo nhưng chưa có commit nào — worktree cần ít nhất một commit. \
             Chạy: git add -A && git commit -m init",
        ),
        _ => ("!", "KHÔNG phải git repo — mọi node sẽ dùng chung thư mục"),
    };
    println!("  {mark} thư mục hiện tại {msg}");
    if missing > 0 {
        println!("\n{missing} agent chưa cài. Node dùng agent đó sẽ thất bại.");
    }
    Ok(())
}

fn which(bin: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join(bin))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}
