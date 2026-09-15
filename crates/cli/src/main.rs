mod tui;
mod web;

use agentgraph_core::agent::{adapter_for, known_agents};
use agentgraph_core::config::{Limits, Plan};
use agentgraph_core::event::EventLog;
use agentgraph_core::graph::{Isolate, NodeSpec};
use agentgraph_core::ids::NodeId;
use agentgraph_core::run::Runner;
use anyhow::Context;
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "agentgraph",
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
    let run_id = agentgraph_core::ids::RunId::generate();
    let dir = root.join(".agentgraph").join("runs").join(run_id.as_str());
    let events = dir.join("events.jsonl");
    let log = EventLog::create(&events)?;
    let mut rx = log.subscribe();

    let runner = Runner::new(&root, limits, log.clone(), run_id).await?;
    // Mặt web phải đăng ký *trước* khi runner chạy: nó dựng trạng thái bằng
    // cách fold event, nên bỏ lỡ run_started/node_added là mất sạch graph.
    let web_state = web.then(|| web::WebState::live(log.clone()));
    let mut handle = tokio::spawn(async move { runner.execute(plan).await });

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
                        agentgraph_core::event::EventKind::RunFinished { .. }
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
        return web::serve(web::WebState::replay(ui.view), port).await;
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

async fn doctor() -> anyhow::Result<()> {
    println!("agentgraph doctor\n");
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
    println!(
        "  {} thư mục hiện tại {}",
        if is_repo { "✓" } else { "!" },
        if is_repo {
            "là git repo — cô lập worktree bật".to_string()
        } else {
            "KHÔNG phải git repo — mọi node sẽ dùng chung thư mục".to_string()
        }
    );
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
