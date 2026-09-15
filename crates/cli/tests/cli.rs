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
