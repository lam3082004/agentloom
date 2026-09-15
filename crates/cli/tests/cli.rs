//! Test lớp vỏ CLI bằng chính binary đã build: những lỗi ở đây (thứ tự đăng
//! ký event, pipe đóng sớm, cờ vô nghĩa) không lộ ra ở tầng thư viện.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_agentgraph");

struct Tmp(std::path::PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// Thư mục tạm KHÔNG phải git repo: worktree tắt, node dùng chung thư mục.
fn tmp() -> Tmp {
    let d = std::env::temp_dir().join(format!("ag-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    Tmp(d)
}

fn viet_plan(dir: &std::path::Path, ten: &str, noi_dung: &str) -> std::path::PathBuf {
    let p = dir.join(ten);
    std::fs::write(&p, noi_dung).unwrap();
    p
}

const PLAN_HAI_NODE: &str = r#"
goal = "mục tiêu tiếng Việt"
[[node]]
id = "a"
title = "node a"
agent = "fake"
task = "việc a"
isolate = "shared"

[[node]]
id = "b"
title = "node b"
agent = "fake"
task = "việc b"
deps = ["a"]
isolate = "shared"
"#;

/// Cổng trống, lấy bằng cách mở rồi đóng ngay.
fn cong_trong() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn http_get(port: u16, path: &str) -> Option<String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut buf = String::new();
    s.read_to_string(&mut buf).ok()?;
    Some(buf)
}

struct Killer(Child);
impl Drop for Killer {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

#[test]
fn co_vo_nghia_bi_chan_ngay_thay_vi_treo() {
    let d = tmp();
    let p = viet_plan(&d.0, "p.toml", PLAN_HAI_NODE);
    for (co, gia_tri) in [
        ("--parallel", "0"),
        ("--timeout-min", "0"),
        ("--budget", "0"),
    ] {
        let t = Instant::now();
        let o = Command::new(BIN)
            .args(["run", p.to_str().unwrap(), "--plain", co, gia_tri])
            .current_dir(&d.0)
            .output()
            .unwrap();
        assert!(!o.status.success(), "{co} {gia_tri} phải là lỗi");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains(co), "thông báo phải chỉ ra cờ sai: {err}");
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "{co} không được treo"
        );
    }
}

#[test]
fn plan_sai_bi_tu_choi_truoc_khi_chay() {
    let d = tmp();
    for (ten, noi_dung, manh) in [
        (
            "trung.toml",
            "goal = \"g\"\n[[node]]\nid=\"a\"\ntitle=\"a\"\nagent=\"fake\"\ntask=\"t\"\n[[node]]\nid=\"a\"\ntitle=\"a\"\nagent=\"fake\"\ntask=\"t\"\n",
            "hai lần",
        ),
        (
            "depma.toml",
            "goal = \"g\"\n[[node]]\nid=\"a\"\ntitle=\"a\"\nagent=\"fake\"\ntask=\"t\"\ndeps=[\"ma\"]\n",
            "không có node đó",
        ),
        (
            "chutrinh.toml",
            "goal = \"g\"\n[[node]]\nid=\"a\"\ntitle=\"a\"\nagent=\"fake\"\ntask=\"t\"\ndeps=[\"b\"]\n[[node]]\nid=\"b\"\ntitle=\"b\"\nagent=\"fake\"\ntask=\"t\"\ndeps=[\"a\"]\n",
            "chu trình",
        ),
    ] {
        let p = viet_plan(&d.0, ten, noi_dung);
        let o = Command::new(BIN)
            .args(["run", p.to_str().unwrap(), "--plain"])
            .current_dir(&d.0)
            .output()
            .unwrap();
        // Trước đây plan hỏng vẫn chạy tới cùng rồi báo "OK" với 0 node.
        assert!(!o.status.success(), "{ten} phải thất bại");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains(manh), "{ten}: thiếu lý do rõ ràng — {err}");
    }
}

#[test]
fn replay_file_khong_ton_tai_bao_ro_ten_file() {
    let d = tmp();
    let o = Command::new(BIN)
        .args(["replay", "/khong/he/co/events.jsonl", "--plain"])
        .current_dir(&d.0)
        .output()
        .unwrap();
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("/khong/he/co/events.jsonl"),
        "phải nói rõ đọc file nào: {err}"
    );
}

#[test]
fn plain_bi_dong_pipe_thi_dung_chu_khong_panic() {
    // `--plain` sinh ra để pipe; `| head` đóng pipe giữa chừng và `println!`
    // panic vì Rust bỏ qua SIGPIPE.
    let d = tmp();
    let p = viet_plan(&d.0, "p.toml", PLAN_HAI_NODE);
    let mut child = Command::new(BIN)
        .args(["run", p.to_str().unwrap(), "--plain"])
        .current_dir(&d.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        // Đọc đúng một dòng rồi đóng đầu đọc, y như `| head -1`.
        let mut r = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert!(line.contains("run_started"), "dòng đầu: {line}");
    }
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).ok();
    child.wait().unwrap();
    assert!(!err.contains("panicked"), "không được panic: {err}");
}

#[test]
fn web_live_thay_du_node_va_van_phuc_vu_sau_khi_graph_xong() {
    let d = tmp();
    let p = viet_plan(&d.0, "p.toml", PLAN_HAI_NODE);
    let port = cong_trong();
    let child = Command::new(BIN)
        .args([
            "run",
            p.to_str().unwrap(),
            "--web",
            "--port",
            &port.to_string(),
        ])
        .current_dir(&d.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _k = Killer(child);

    // Chờ graph chạy xong rồi mới hỏi: web phải sống lâu hơn graph.
    let het = Instant::now() + Duration::from_secs(30);
    let mut body = String::new();
    while Instant::now() < het {
        std::thread::sleep(Duration::from_millis(300));
        let Some(r) = http_get(port, "/api/view") else {
            continue;
        };
        if r.contains("\"finished\":true") {
            body = r;
            break;
        }
        body = r;
    }
    assert!(
        body.contains("\"finished\":true"),
        "web phải còn phục vụ sau khi graph xong: {body}"
    );
    // Mặt web đăng ký sau khi runner chạy thì mất sạch node và goal.
    assert!(body.contains("node a"), "thiếu node trong view: {body}");
    assert!(
        body.contains("mục tiêu tiếng Việt"),
        "thiếu goal (hoặc hỏng tiếng Việt): {body}"
    );

    let r404 = http_get(port, "/khong-co-trang-nay").unwrap_or_default();
    assert!(r404.starts_with("HTTP/1.1 404"), "phải là 404: {r404}");
}

/// Log tối giản, viết tay từng dòng NDJSON đúng schema `Event` — đủ để test
/// nhánh đọc log/tìm session/báo lỗi của `ask` mà không cần chạy agent thật.
/// `node_finished` không có khoá `session` mô phỏng đúng log CŨ (trước khi
/// việc 3 thêm trường này) — `#[serde(default)]` phải đọc được, không vỡ.
fn ghi_log_toi_gian(
    dir: &std::path::Path,
    node: &str,
    workspace: Option<&std::path::Path>,
    session: Option<&str>,
) -> std::path::PathBuf {
    let p = dir.join("events.jsonl");
    let mut lines = vec![format!(
        r#"{{"seq":1,"at":"2024-01-01T00:00:00Z","kind":"run_started","goal":"g","run":"r1"}}"#
    )];
    lines.push(format!(
        r#"{{"seq":2,"at":"2024-01-01T00:00:01Z","node":"{node}","kind":"node_added","title":"{node}","agent":"fake","deps":[],"by":"plan"}}"#
    ));
    if let Some(ws) = workspace {
        lines.push(format!(
            r#"{{"seq":3,"at":"2024-01-01T00:00:02Z","node":"{node}","kind":"workspace","action":"worktree","path":"{}"}}"#,
            ws.display()
        ));
    }
    let session_field = session
        .map(|s| format!(r#","session":"{s}""#))
        .unwrap_or_default();
    lines.push(format!(
        r#"{{"seq":4,"at":"2024-01-01T00:00:03Z","node":"{node}","kind":"node_finished","ok":true,"cost_usd":0.0,"tokens_in":0,"tokens_out":0,"summary":"xong"{session_field}}}"#
    ));
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    p
}

#[test]
fn ask_bao_ro_khi_node_khong_ton_tai() {
    let d = tmp();
    let events = ghi_log_toi_gian(&d.0, "a", Some(&d.0), Some("sess-1"));
    let o = Command::new(BIN)
        .args(["ask", events.to_str().unwrap(), "khong-co", "hỏi gì đó"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("khong-co"), "phải nêu rõ node nào: {err}");
}

#[test]
fn ask_bao_ro_khi_node_chua_co_session() {
    let d = tmp();
    // node_finished KHÔNG mang session — giống log cũ trước việc 3, hoặc node
    // hỏng trước khi agent kịp trả về session_id.
    let events = ghi_log_toi_gian(&d.0, "a", Some(&d.0), None);
    let o = Command::new(BIN)
        .args(["ask", events.to_str().unwrap(), "a", "hỏi gì đó"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("session"), "phải nói rõ thiếu session: {err}");
}

#[test]
fn ask_bao_ro_khi_worktree_da_bi_xoa() {
    let d = tmp();
    let da_xoa = d.0.join("worktree-khong-con");
    let events = ghi_log_toi_gian(&d.0, "a", Some(&da_xoa), Some("sess-1"));
    let o = Command::new(BIN)
        .args(["ask", events.to_str().unwrap(), "a", "hỏi gì đó"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("xoá") || err.contains("worktree"),
        "phải nói rõ worktree đã mất: {err}"
    );
}

/// `doctor` từng báo "✓ cô lập worktree bật" trong repo vừa `git init` — đúng
/// tình huống worktree không tạo nổi. Người mới tin dấu ✓ rồi vấp ngay lượt đầu.
#[test]
fn doctor_canh_bao_repo_chua_co_commit() {
    let d = tmp();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(&d.0)
        .output()
        .unwrap();
    let o = Command::new(BIN)
        .arg("doctor")
        .current_dir(&d.0)
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("chưa có commit"), "phải cảnh báo: {out}");
    assert!(
        !out.contains("cô lập worktree bật"),
        "không được báo ✓ sai: {out}"
    );
}

#[test]
fn runs_khong_co_thu_muc_thi_bao_ro_chu_khong_loi() {
    let d = tmp();
    let o = Command::new(BIN)
        .args(["runs", "--root", d.0.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(o.status.success(), "chưa từng chạy lần nào không phải lỗi");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("chưa có lượt chạy"), "phải nói rõ: {out}");
}

#[test]
fn runs_liet_ke_moi_nhat_truoc_va_khong_chet_vi_log_do_dang() {
    let d = tmp();
    let runs_dir = d.0.join(".agentgraph").join("runs");
    // Run cũ: xong sạch, có run_finished.
    let cu = runs_dir.join("20240101-000000-aaaaaa");
    std::fs::create_dir_all(&cu).unwrap();
    std::fs::write(
        cu.join("events.jsonl"),
        [
            r#"{"seq":1,"at":"2024-01-01T00:00:00Z","kind":"run_started","goal":"muc tieu cu","run":"20240101-000000-aaaaaa"}"#,
            r#"{"seq":2,"at":"2024-01-01T00:00:01Z","node":"a","kind":"node_added","title":"a","agent":"fake","deps":[],"by":"plan"}"#,
            r#"{"seq":3,"at":"2024-01-01T00:00:02Z","node":"a","kind":"node_state","state":"done"}"#,
            r#"{"seq":4,"at":"2024-01-01T00:00:03Z","kind":"run_finished","ok":true,"total_cost_usd":0.5}"#,
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();

    // Run mới hơn: dở dang — bị giết giữa chừng, không có run_finished.
    let moi = runs_dir.join("20240102-000000-bbbbbb");
    std::fs::create_dir_all(&moi).unwrap();
    std::fs::write(
        moi.join("events.jsonl"),
        [
            r#"{"seq":1,"at":"2024-01-02T00:00:00Z","kind":"run_started","goal":"muc tieu moi","run":"20240102-000000-bbbbbb"}"#,
            r#"{"seq":2,"at":"2024-01-02T00:00:01Z","node":"a","kind":"node_added","title":"a","agent":"fake","deps":[],"by":"plan"}"#,
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();

    let o = Command::new(BIN)
        .args(["runs", "--root", d.0.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(o.status.success(), "log dở dang không được làm lệnh chết");
    let out = String::from_utf8_lossy(&o.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "phải liệt kê đúng 2 run: {out}");
    assert!(
        lines[0].contains("20240102-000000-bbbbbb"),
        "mới nhất phải đứng trước: {out}"
    );
    assert!(lines[0].contains("DỞ DANG"), "run chưa xong: {out}");
    assert!(lines[1].contains("20240101-000000-aaaaaa"));
    assert!(lines[1].contains("OK"), "run xong sạch: {out}");
    assert!(lines[1].contains("muc tieu cu"));
}

/// Huỷ giữa chừng hoặc chạm trần ngân sách để lại node chưa từng chạy. Không
/// đếm chúng thì tổng hiển thị ít hơn số node trong plan và người đọc tưởng
/// plan nhỏ hơn thực tế.
#[test]
fn runs_dem_ca_node_chua_chay() {
    let d = tmp();
    let run =
        d.0.join(".agentgraph")
            .join("runs")
            .join("20240103-000000-cccccc");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(
        run.join("events.jsonl"),
        [
            r#"{"seq":1,"at":"2024-01-03T00:00:00Z","kind":"run_started","goal":"bi huy","run":"20240103-000000-cccccc"}"#,
            r#"{"seq":2,"at":"2024-01-03T00:00:01Z","node":"a","kind":"node_added","title":"a","agent":"fake","deps":[],"by":"plan"}"#,
            r#"{"seq":3,"at":"2024-01-03T00:00:01Z","node":"b","kind":"node_added","title":"b","agent":"fake","deps":["a"],"by":"plan"}"#,
            r#"{"seq":4,"at":"2024-01-03T00:00:01Z","node":"c","kind":"node_added","title":"c","agent":"fake","deps":["a"],"by":"plan"}"#,
            r#"{"seq":5,"at":"2024-01-03T00:00:02Z","node":"a","kind":"node_state","state":"running"}"#,
            r#"{"seq":6,"at":"2024-01-03T00:00:03Z","node":"a","kind":"node_state","state":"failed"}"#,
            r#"{"seq":7,"at":"2024-01-03T00:00:03Z","kind":"run_finished","ok":false,"total_cost_usd":0.0}"#,
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let o = Command::new(BIN)
        .args(["runs", "--root", d.0.to_str().unwrap()])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("1 hỏng"), "{out}");
    assert!(
        out.contains("2 chưa chạy"),
        "b và c chưa từng chạy phải được đếm: {out}"
    );
}

/// Ctrl-C thật (SIGINT) khi graph đã xong nhanh (không còn gì để giết) không
/// được bắt tiến trình `--web` ngủ đủ 3 giây cố định trước khi thoát — đó là
/// độ trễ vô ích áp cho MỌI lần huỷ, kể cả khi chẳng có gì phải chờ.
#[test]
fn sigint_web_khong_ngu_co_dinh_khi_khong_con_gi_de_giet() {
    let d = tmp();
    // Plan không SLEEP: hai node fake xong gần như tức khắc.
    let p = viet_plan(&d.0, "p.toml", PLAN_HAI_NODE);
    let port = cong_trong();
    let mut child = Command::new(BIN)
        .args([
            "run",
            p.to_str().unwrap(),
            "--web",
            "--port",
            &port.to_string(),
        ])
        .current_dir(&d.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();

    // Chờ graph xong hẳn (web vẫn mở phục vụ) trước khi gửi SIGINT — đúng
    // kịch bản "mọi thứ đã xong, không còn gì phải giết".
    let het = Instant::now() + Duration::from_secs(30);
    let mut thay_xong = false;
    while Instant::now() < het {
        if let Some(body) = http_get(port, "/api/view") {
            if body.contains("\"finished\":true") {
                thay_xong = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(thay_xong, "graph phải xong trong 30s để test có ý nghĩa");

    let t = Instant::now();
    Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .unwrap();
    let status = child.wait().unwrap();
    let elapsed = t.elapsed();

    assert!(!status.success(), "SIGINT phải làm tiến trình thoát khác 0");
    // Không còn agent nào đang chạy lúc huỷ -> không có lý do gì phải chờ
    // hết 3 giây cố định. Cho dư ra một chút cho CI chậm, nhưng phải rõ
    // ràng dưới trần cũ.
    assert!(
        elapsed < Duration::from_millis(2500),
        "SIGINT lúc không còn gì để giết mà vẫn mất {elapsed:?} — đang ngủ cố định thay vì chờ việc thật"
    );
}

#[test]
fn ask_bao_ro_khi_file_log_khong_ton_tai() {
    let o = Command::new(BIN)
        .args(["ask", "/khong/he/co/events.jsonl", "a", "hỏi gì đó"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("/khong/he/co/events.jsonl"),
        "phải nói rõ đọc file nào: {err}"
    );
}
