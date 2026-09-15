//! Test toàn bộ đường ống: plan -> lập lịch -> chạy song song -> join ->
//! mutation động. Dùng adapter giả nên không tốn token và không cần mạng.

use agentgraph_core::config::{Limits, Plan};
use agentgraph_core::event::{EventKind, EventLog};
use agentgraph_core::graph::NodeState;
use agentgraph_core::ids::NodeId;
use agentgraph_core::run::Runner;

struct Tmp(std::path::PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn tmp() -> Tmp {
    let d = std::env::temp_dir().join(format!("ag-run-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    Tmp(d)
}

fn plan(s: &str) -> Plan {
    Plan::from_toml(s).unwrap()
}

async fn run(
    dir: &std::path::Path,
    p: Plan,
) -> (
    agentgraph_core::run::RunSummary,
    Vec<agentgraph_core::event::Event>,
) {
    run_with(dir, p, 4).await
}

async fn run_with(
    dir: &std::path::Path,
    p: Plan,
    parallel: usize,
) -> (
    agentgraph_core::run::RunSummary,
    Vec<agentgraph_core::event::Event>,
) {
    let log = EventLog::create(dir.join("events.jsonl")).unwrap();
    let r = Runner::new(
        dir,
        Limits {
            max_parallel: parallel,
            ..Default::default()
        },
        log.clone(),
        agentgraph_core::ids::RunId::generate(),
    )
    .await
    .unwrap();
    let s = r.execute(p).await.unwrap();
    let ev = EventLog::replay(dir.join("events.jsonl")).unwrap();
    (s, ev)
}

#[tokio::test]
async fn fan_out_roi_join_chay_dung_thu_tu() {
    let d = tmp();
    let (s, ev) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "việc a"
isolate = "shared"

[[node]]
id = "b"
title = "b"
agent = "fake"
task = "việc b"
isolate = "shared"

[[node]]
id = "j"
title = "join"
agent = "fake"
task = "gộp"
deps = ["a", "b"]
isolate = "shared"
"#,
        ),
    )
    .await;
    assert!(s.ok, "phải xong sạch");
    assert_eq!(s.done, 3);

    // j chỉ được chạy sau khi cả a lẫn b đã xong.
    let running_j = ev
        .iter()
        .position(|e| {
            e.node.as_ref().map(|n| n.as_str()) == Some("j")
                && matches!(&e.kind, EventKind::NodeState { state } if state == "running")
        })
        .expect("j phải có lúc chạy");
    for dep in ["a", "b"] {
        let done = ev
            .iter()
            .position(|e| {
                e.node.as_ref().map(|n| n.as_str()) == Some(dep)
                    && matches!(&e.kind, EventKind::NodeState { state } if state == "done")
            })
            .unwrap();
        assert!(done < running_j, "{dep} phải xong trước khi j chạy");
    }
}

#[tokio::test]
async fn agent_spawn_node_moi_luc_dang_chay() {
    let d = tmp();
    let (s, ev) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "root"
title = "root"
agent = "fake"
task = """việc chính EMIT:{"op":"spawn","id":"con","agent":"fake","task":"việc con"}"""
isolate = "shared"
"#,
        ),
    )
    .await;
    // Graph mọc từ 1 lên 2 node, node thứ hai do agent đẻ ra.
    assert_eq!(s.done, 2, "node do agent spawn phải được chạy");
    let added_by_agent = ev.iter().any(|e| {
        matches!(&e.kind, EventKind::NodeAdded { by, .. }
            if *by == agentgraph_core::event::Origin::Agent)
    });
    assert!(added_by_agent, "phải ghi nhận node sinh ra bởi agent");
    assert!(s.ok);
}

#[tokio::test]
async fn mutation_khong_hop_le_bi_tu_choi_va_van_duoc_ghi_lai() {
    let d = tmp();
    let (s, ev) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "root"
title = "root"
agent = "fake"
task = """x EMIT:{"op":"spawn","id":"../../escape","agent":"fake","task":"t"}"""
isolate = "shared"
"#,
        ),
    )
    .await;
    assert_eq!(s.done, 1, "chỉ node gốc chạy, mutation xấu không tạo node");
    let rejected = ev
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::Mutation {
                accepted: false,
                reason,
                ..
            } => Some(reason.clone()),
            _ => None,
        })
        .expect("phải có bản ghi từ chối");
    assert!(rejected.contains("id không hợp lệ"), "lý do: {rejected}");
}

#[tokio::test]
async fn node_hong_thi_nhanh_sau_bi_bo_qua_chu_khong_chay_mu() {
    let d = tmp();
    let (s, _) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "FAIL việc này hỏng"
isolate = "shared"

[[node]]
id = "b"
title = "b"
agent = "fake"
task = "phụ thuộc a"
deps = ["a"]
isolate = "shared"
"#,
        ),
    )
    .await;
    assert_eq!(s.failed, 1);
    assert_eq!(s.skipped, 1, "b phải bị bỏ qua, không được chạy");
    assert!(!s.ok);
}

#[tokio::test]
async fn agent_khong_ton_tai_lam_node_that_bai_chu_khong_treo() {
    let d = tmp();
    let log = EventLog::create(d.0.join("events.jsonl")).unwrap();
    let mut r = Runner::new(
        &d.0,
        Limits::default(),
        log,
        agentgraph_core::ids::RunId::generate(),
    )
    .await
    .unwrap();
    // Đi thẳng vào graph để vượt qua kiểm tra của mutation.
    r.graph
        .add(agentgraph_core::graph::NodeSpec {
            id: NodeId::new("x").unwrap(),
            title: "x".into(),
            agent: "khong-ton-tai".into(),
            task: "t".into(),
            deps: vec![],
            model: None,
            verify: None,
            isolate: agentgraph_core::graph::Isolate::Shared,
        })
        .unwrap();
    let s = r
        .execute(Plan {
            goal: "g".into(),
            nodes: vec![],
        })
        .await
        .unwrap();
    assert_eq!(s.failed, 1);
    assert!(s.graph_settled());
}

trait Settled {
    fn graph_settled(&self) -> bool;
}
impl Settled for agentgraph_core::run::RunSummary {
    fn graph_settled(&self) -> bool {
        self.done + self.failed + self.skipped > 0
    }
}

#[tokio::test]
async fn state_cuoi_dung_kien_state_replay_tu_log() {
    let d = tmp();
    let (_, ev) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "x"
isolate = "shared"
"#,
        ),
    )
    .await;
    // Log phải đủ để dựng lại trạng thái cuối mà không cần bộ nhớ tiến trình.
    let last = ev
        .iter()
        .filter(|e| e.node.as_ref().map(|n| n.as_str()) == Some("a"))
        .filter_map(|e| match &e.kind {
            EventKind::NodeState { state } => Some(state.clone()),
            _ => None,
        })
        .next_back();
    assert_eq!(last.as_deref(), Some("done"));
    assert_eq!(NodeState::Done.as_str(), "done");
}

#[tokio::test]
async fn verifier_phu_quyet_duoc_agent_bao_xong() {
    let d = tmp();
    // Agent giả luôn báo "xong". Verifier kiểm tra một file agent không hề tạo.
    let (s, ev) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "giả vờ làm xong"
verify = "test -f khong-he-ton-tai.txt"
isolate = "shared"
"#,
        ),
    )
    .await;
    assert_eq!(
        s.failed, 1,
        "agent nói xong nhưng verifier đỏ thì node phải HỎNG"
    );
    assert!(!s.ok);
    let note = ev.iter().any(|e| {
        matches!(&e.kind,
        EventKind::Note { text } if text.contains("verifier ĐỎ"))
    });
    assert!(note, "phải ghi lại verifier đỏ");
}

#[tokio::test]
async fn verifier_xanh_thi_node_xong_that() {
    let d = tmp();
    let (s, _) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "x"
verify = "true"
isolate = "shared"
"#,
        ),
    )
    .await;
    assert_eq!(s.done, 1);
    assert!(s.ok);
}

#[tokio::test]
async fn node_hong_vi_verifier_van_chan_nhanh_phia_sau() {
    let d = tmp();
    let (s, _) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "x"
verify = "false"
isolate = "shared"

[[node]]
id = "b"
title = "b"
agent = "fake"
task = "y"
deps = ["a"]
isolate = "shared"
"#,
        ),
    )
    .await;
    assert_eq!(s.failed, 1);
    assert_eq!(s.skipped, 1);
}

#[tokio::test]
async fn parallel_0_khong_lam_treo_vong_lap_lich() {
    // Trần song song 0 từng làm vòng lập lịch quay vô hạn: không nạp được node
    // nào nhưng vẫn còn node ready nên không bao giờ thoát.
    let d = tmp();
    let f = run_with(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "việc a"
isolate = "shared"
"#,
        ),
        0,
    );
    let (s, _) = tokio::time::timeout(std::time::Duration::from_secs(20), f)
        .await
        .expect("không được treo khi max_parallel = 0");
    assert_eq!(s.done, 1);
}

#[tokio::test]
async fn dep_khai_bao_sau_van_duoc_nap_chu_khong_bi_bo_roi() {
    // Plan liệt kê node con trước node cha là hợp lệ; trước đây node con bị
    // `Graph::add` từ chối vì dep chưa có, rồi lượt chạy vẫn báo OK.
    let d = tmp();
    let (s, _) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "sau"
title = "sau"
agent = "fake"
task = "việc sau"
deps = ["truoc"]
isolate = "shared"

[[node]]
id = "truoc"
title = "trước"
agent = "fake"
task = "việc trước"
isolate = "shared"
"#,
        ),
    )
    .await;
    assert!(s.ok);
    assert_eq!(s.done, 2, "cả hai node phải chạy");
}

#[tokio::test]
async fn node_trong_plan_bi_tu_choi_thi_luot_chay_khong_duoc_bao_ok() {
    // Dep ma: node không bao giờ chạy. Báo "OK" ở đây là kiểu sai nguy hiểm
    // nhất — người dùng tưởng plan đã chạy hết.
    let d = tmp();
    let (s, ev) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "việc a"
deps = ["khong-co"]
isolate = "shared"
"#,
        ),
    )
    .await;
    assert!(!s.ok, "plan không nạp được thì không thể là OK");
    assert_eq!(s.done, 0);
    assert!(ev.iter().any(|e| matches!(&e.kind,
        EventKind::Mutation { op, accepted, .. } if op == "plan_add" && !accepted)));
}

#[tokio::test]
async fn cham_tran_ngan_sach_thi_dung_lai_chu_khong_quay_vong() {
    // Trần 0 USD: không node nào được nạp. Vòng lập lịch phải thoát chứ không
    // quay tại chỗ, và kết quả không được coi là OK.
    let d = tmp();
    let log = EventLog::create(d.0.join("events.jsonl")).unwrap();
    let r = Runner::new(
        &d.0,
        Limits {
            max_total_usd: 0.0,
            ..Default::default()
        },
        log,
        agentgraph_core::ids::RunId::generate(),
    )
    .await
    .unwrap();
    let s = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        r.execute(plan(
            r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "việc a"
isolate = "shared"
"#,
        )),
    )
    .await
    .expect("chạm trần ngân sách không được treo")
    .unwrap();
    assert!(!s.ok, "không chạy được node nào thì không phải OK");
    assert_eq!(s.done, 0);
}

#[tokio::test]
async fn verifier_treo_khong_lam_dung_ca_luot_chay() {
    // Verifier chạy lệnh tuỳ ý của người dùng; một lệnh không bao giờ thoát
    // từng làm orchestrator đứng vĩnh viễn vì trần thời gian chỉ áp cho agent.
    let d = tmp();
    let log = EventLog::create(d.0.join("events.jsonl")).unwrap();
    let r = Runner::new(
        &d.0,
        Limits {
            node_timeout: std::time::Duration::from_secs(2),
            ..Default::default()
        },
        log,
        agentgraph_core::ids::RunId::generate(),
    )
    .await
    .unwrap();
    let s = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        r.execute(plan(
            r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "việc a"
verify = "sleep 600"
isolate = "shared"
"#,
        )),
    )
    .await
    .expect("verifier treo không được làm treo cả lượt chạy")
    .unwrap();
    assert_eq!(s.failed, 1, "verifier hết giờ thì node phải HỎNG");
}

#[tokio::test]
async fn verifier_het_gio_thi_tien_trinh_con_cung_bi_giet() {
    // Giết mỗi `sh` để lại lệnh thật chạy mồ côi (dev server, test runner)
    // suốt phần đời còn lại của máy.
    let d = tmp();
    let dau_vet = d.0.join("verifier-van-song.txt");
    let log = EventLog::create(d.0.join("events.jsonl")).unwrap();
    let r = Runner::new(
        &d.0,
        Limits {
            node_timeout: std::time::Duration::from_secs(1),
            ..Default::default()
        },
        log,
        agentgraph_core::ids::RunId::generate(),
    )
    .await
    .unwrap();
    let s = r
        .execute(plan(&format!(
            r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "việc a"
verify = "sh -c 'sleep 4; touch {}' & sleep 60"
isolate = "shared"
"#,
            dau_vet.display()
        )))
        .await
        .unwrap();
    assert_eq!(s.failed, 1);
    // Đợi qua mốc lệnh kia định chạm vào đĩa; còn sống thì nó sẽ tạo file.
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    assert!(
        !dau_vet.exists(),
        "lệnh verify phải bị giết cùng nhóm, không được chạy tiếp"
    );
}

/// Trong một git repo thật, mỗi node có worktree riêng. Node B phụ thuộc A
/// thì PHẢI thấy việc A đã làm — nếu không, mọi chuỗi "làm rồi review" đều vô
/// nghĩa vì người review nhìn vào cây trống.
#[tokio::test]
async fn node_sau_phai_thay_viec_cua_node_truoc_trong_chuoi_mot_dep() {
    let d = tmp();
    // Phải là git repo thì worktree mới bật; thư mục thường sẽ dùng chung cây
    // và test này sẽ xanh giả.
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "a@b"],
        vec!["config", "user.name", "t"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(&d.0)
            .output()
            .unwrap();
    }
    std::fs::write(d.0.join("README.md"), "x").unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-qm", "init"]] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(&d.0)
            .output()
            .unwrap();
    }

    let (s, _) = run(
        &d.0,
        plan(
            r#"
goal = "chuoi"
[[node]]
id = "a"
title = "a tạo file"
agent = "fake"
task = "WRITE:tu-a.txt:ALPHA"

[[node]]
id = "b"
title = "b phải thấy file của a"
agent = "fake"
task = "đọc"
deps = ["a"]
verify = "test -f tu-a.txt"
"#,
        ),
    )
    .await;
    assert_eq!(s.failed, 0, "node b không thấy việc của node a");
    assert_eq!(s.done, 2);
}

/// Nhánh graph do agent tự mọc ra cũng phải chịu verifier như node trong plan.
/// Nếu không, "verifier có quyền phủ quyết" chỉ đúng với phần graph người viết.
#[tokio::test]
async fn node_do_agent_spawn_cung_bi_verifier_phu_quyet() {
    let d = tmp();
    let (s, ev) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "root"
title = "root"
agent = "fake"
task = """x EMIT:{"op":"spawn","id":"con","agent":"fake","task":"giả vờ xong","verify":"false"}"""
isolate = "shared"
"#,
        ),
    )
    .await;
    assert_eq!(s.done, 1, "chỉ root xong");
    assert_eq!(
        s.failed, 1,
        "node con có verify đỏ phải HỎNG dù agent báo xong"
    );
    assert!(ev.iter().any(|e| {
        e.node.as_ref().map(|n| n.as_str()) == Some("con")
            && matches!(&e.kind, EventKind::Note { text } if text.contains("verifier ĐỎ"))
    }));
}

#[tokio::test]
async fn node_do_agent_spawn_khong_co_verify_thi_phai_noi_ro() {
    let d = tmp();
    let (s, ev) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "root"
title = "root"
agent = "fake"
task = """x EMIT:{"op":"spawn","id":"con","agent":"fake","task":"t"}"""
isolate = "shared"
"#,
        ),
    )
    .await;
    assert_eq!(s.done, 2);
    // Không chặn — nhưng việc chỉ tin lời agent phải hiện ra trong log.
    assert!(ev.iter().any(|e| {
        e.node.as_ref().map(|n| n.as_str()) == Some("con")
            && matches!(&e.kind, EventKind::Note { text } if text.contains("không có verify"))
    }));
}

/// Kiểm qua `View` — tức đúng thứ TUI và web nhìn thấy — chứ không chỉ qua
/// event log. Dòng log gửi tới node chưa được `NodeAdded` sẽ bị fold bỏ qua.
#[tokio::test]
async fn canh_bao_va_dep_cua_node_spawn_hien_dung_tren_giao_dien() {
    let d = tmp();
    let (_, ev) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "khac"
title = "khac"
agent = "fake"
task = "x"
isolate = "shared"

[[node]]
id = "root"
title = "root"
agent = "fake"
task = """x EMIT:{"op":"spawn","id":"con","agent":"fake","task":"t","after":["khac"]}"""
deps = ["khac"]
isolate = "shared"
"#,
        ),
    )
    .await;
    let mut v = agentgraph_core::view::View::default();
    for e in &ev {
        v.apply(e);
    }
    let con = &v.nodes["con"];
    assert!(
        con.lines.iter().any(|l| l.contains("không có verify")),
        "cảnh báo phải hiện trên node con, log node: {:?}",
        con.lines
    );
    let mut deps = con.deps.clone();
    deps.sort();
    assert_eq!(
        deps,
        vec!["khac".to_string(), "root".to_string()],
        "dep hiển thị phải khớp graph thật"
    );
}

fn con_song(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Huỷ giữa chừng (canceller().send(true), tương ứng q trong TUI / Ctrl-C)
/// không được để lại tiến trình agent mồ côi. FakeAdapter với `SLEEP:` spawn
/// một `sleep` THẬT nên PID kiểm được bằng `kill -0`, không chỉ giả lập trong
/// bộ nhớ Rust.
#[tokio::test]
async fn huy_giua_chung_giet_tien_trinh_agent_khong_de_mo_coi() {
    let d = tmp();
    let log = EventLog::create(d.0.join("events.jsonl")).unwrap();
    let mut rx = log.subscribe();
    let r = Runner::new(
        &d.0,
        Limits {
            max_parallel: 1,
            ..Default::default()
        },
        log.clone(),
        agentgraph_core::ids::RunId::generate(),
    )
    .await
    .unwrap();
    let canceller = r.canceller();
    let handle = tokio::spawn(r.execute(plan(
        r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "SLEEP:600"
isolate = "shared"
"#,
    )));

    // Đợi tới khi fake đã spawn `sleep` thật và log lại PID của nó.
    let pid: u32 = loop {
        match rx.recv().await.unwrap().kind {
            EventKind::Note { text } if text.starts_with("fake-pid:") => {
                break text.strip_prefix("fake-pid:").unwrap().parse().unwrap();
            }
            _ => continue,
        }
    };
    assert!(
        con_song(pid),
        "tiến trình sleep phải đang chạy trước khi huỷ"
    );

    canceller.send(true).unwrap();
    let s = tokio::time::timeout(std::time::Duration::from_secs(10), handle)
        .await
        .expect("huỷ không được làm treo cả lượt chạy")
        .unwrap()
        .unwrap();

    assert_eq!(
        s.failed, 1,
        "node đang chạy khi bị huỷ phải kết thúc HỎNG, không kẹt ở running"
    );
    assert!(!s.ok, "lượt chạy bị huỷ không được báo OK");

    // Cho `kill -KILL` kịp có hiệu lực rồi kiểm PID thật đã biến mất.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !con_song(pid),
        "tiến trình sleep phải bị giết, không được mồ côi"
    );

    let ev = EventLog::replay(d.0.join("events.jsonl")).unwrap();
    assert!(
        ev.iter()
            .any(|e| matches!(&e.kind, EventKind::RunFinished { .. })),
        "run_finished phải được ghi dù bị huỷ giữa chừng"
    );
    let last_state = ev
        .iter()
        .filter(|e| e.node.as_ref().map(|n| n.as_str()) == Some("a"))
        .filter_map(|e| match &e.kind {
            EventKind::NodeState { state } => Some(state.clone()),
            _ => None,
        })
        .next_back();
    assert_ne!(
        last_state.as_deref(),
        Some("running"),
        "node không được kẹt ở trạng thái running trong event log"
    );
}

/// Huỷ giữa chừng khi node đang chạy VERIFIER (không phải agent) phải giết
/// verifier ngay, không chờ tới hết `node_timeout`. Agent (fake) xong gần
/// tức khắc rồi verify chạy `sleep`; ta huỷ trong lúc đó và verifier phải
/// bị giết cùng lúc — không phải sau khi tự hết giờ.
#[tokio::test]
async fn huy_giua_chung_giet_ca_verifier_dang_chay() {
    let d = tmp();
    let dau_vet = d.0.join("verifier-van-song.txt");
    let log = EventLog::create(d.0.join("events.jsonl")).unwrap();
    let mut rx = log.subscribe();
    let r = Runner::new(
        &d.0,
        Limits {
            max_parallel: 1,
            // Cố tình đặt trần thời gian lớn hơn hẳn thời gian test chờ:
            // nếu huỷ không giết được verifier, orchestrator sẽ đợi tới đây
            // thay vì đợi tín hiệu huỷ, lộ đúng bug đang kiểm.
            node_timeout: std::time::Duration::from_secs(20),
            ..Default::default()
        },
        log.clone(),
        agentgraph_core::ids::RunId::generate(),
    )
    .await
    .unwrap();
    let canceller = r.canceller();
    let handle = tokio::spawn(r.execute(plan(&format!(
        r#"
goal = "g"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "việc a"
verify = "sh -c 'sleep 3; touch {}' & sleep 30"
isolate = "shared"
"#,
        dau_vet.display()
    ))));

    // Đợi verifier chắc chắn đã bắt đầu chạy trước khi huỷ.
    loop {
        match rx.recv().await.unwrap().kind {
            EventKind::NodeState { state } if state == "running" => break,
            _ => continue,
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    canceller.send(true).unwrap();

    // Huỷ khi verifier đang chạy phải kết thúc lượt chạy gần như ngay —
    // không phải chờ tới `node_timeout` (20s) hay hết `sleep 30`.
    let s = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("huỷ lúc verifier đang chạy phải giết verifier ngay, không chờ hết giờ")
        .unwrap()
        .unwrap();
    assert_eq!(s.failed, 1, "node có verifier bị huỷ giữa chừng phải HỎNG");
    assert!(!s.ok);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        !dau_vet.exists(),
        "verifier phải bị giết khi huỷ, không được chạy tiếp tới lúc ghi file"
    );
}

/// Hai node song song ghi CÙNG một file với nội dung khác nhau; node thứ ba
/// phụ thuộc cả hai thì merge nhánh thứ hai vào worktree của nó sẽ đụng độ.
/// Trước đây orchestrator chỉ `git merge --abort` rồi ghi Note vào log — agent
/// của node thứ ba không hề biết, chạy tiếp trên cây thiếu việc của một dep.
#[tokio::test]
async fn node_sau_duoc_bao_dung_do_merge_qua_protocol() {
    let d = tmp();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "a@b"],
        vec!["config", "user.name", "t"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(&d.0)
            .output()
            .unwrap();
    }
    std::fs::write(d.0.join("README.md"), "x").unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-qm", "init"]] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(&d.0)
            .output()
            .unwrap();
    }

    let (_, ev) = run(
        &d.0,
        plan(
            r#"
goal = "dung do"
[[node]]
id = "a"
title = "a"
agent = "fake"
task = "WRITE:cung-file.txt:tu-a"

[[node]]
id = "b"
title = "b"
agent = "fake"
task = "WRITE:cung-file.txt:tu-b"

[[node]]
id = "c"
title = "c"
agent = "fake"
task = "phu thuoc ca hai"
deps = ["a", "b"]
"#,
        ),
    )
    .await;

    let note = ev.iter().find_map(|e| match &e.kind {
        EventKind::Note { text } if text.contains("merge đụng độ") => Some(text.clone()),
        _ => None,
    });
    assert!(note.is_some(), "phải có Note về đụng độ merge");

    // Protocol chỉ lộ ra qua FakeAdapter — nó ghi lại nguyên văn cái nó nhận
    // được, đúng như claude/codex thật sẽ nhận qua --append-system-prompt.
    let protocol_cua_c = ev
        .iter()
        .find_map(|e| {
            if e.node.as_ref().map(|n| n.as_str()) != Some("c") {
                return None;
            }
            match &e.kind {
                EventKind::Note { text } if text.starts_with("[fake:protocol]") => {
                    Some(text.clone())
                }
                _ => None,
            }
        })
        .expect("phải log lại protocol mà node c nhận được");
    assert!(
        protocol_cua_c.contains("ĐỤNG ĐỘ"),
        "protocol của c phải cảnh báo đụng độ merge: {protocol_cua_c}"
    );
    assert!(
        protocol_cua_c.contains("ag/") && protocol_cua_c.contains("/b"),
        "protocol phải nêu tên branch bị đụng độ: {protocol_cua_c}"
    );
}

/// Ở chế độ `shared` mọi node dùng chung một file mutation. Mỗi dòng chỉ được
/// áp đúng một lần — nếu không, node chạy sau sẽ áp lại mutation của node
/// trước và nhận nhầm mình là cha của node spawn.
#[tokio::test]
async fn shared_mode_moi_dong_mutation_chi_ap_mot_lan() {
    let d = tmp();
    let (_, ev) = run(
        &d.0,
        plan(
            r#"
goal = "g"
[[node]]
id = "truoc"
title = "truoc"
agent = "fake"
task = """x EMIT:{"op":"write_memory","key":"k","value":"v"}"""
isolate = "shared"

[[node]]
id = "sau"
title = "sau"
agent = "fake"
task = "y"
deps = ["truoc"]
isolate = "shared"
"#,
        ),
    )
    .await;
    let lan_ap: Vec<String> = ev
        .iter()
        .filter(|e| matches!(&e.kind, EventKind::Mutation { op, .. } if op == "write_memory"))
        .map(|e| e.node.as_ref().map(|n| n.to_string()).unwrap_or_default())
        .collect();
    assert_eq!(
        lan_ap,
        vec!["truoc".to_string()],
        "mutation bị áp bởi: {lan_ap:?}"
    );
}
