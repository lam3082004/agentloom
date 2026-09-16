//! Dọn rác của các lượt chạy cũ: worktree, branch, event log.
//!
//! Mỗi node để lại một worktree (`.agentloom/worktrees/<run>/<node>`) và một
//! branch (`al/<run>/<node>`). Chúng CỐ Ý không tự xoá: người dùng còn review,
//! merge, `ask` lại node cũ. Nhưng chạy vài chục lượt là repo đầy worktree, và
//! cách dọn tay trong README (`git worktree remove --force` cho mọi thư mục)
//! xoá luôn cả việc agent làm mà chưa ai xem.
//!
//! Nên module này tách làm hai nửa: [`scan`] chỉ nhìn và trả về hiện trạng,
//! [`clean`] mới xoá. Giao diện gọi `scan` trước để hỏi người dùng — mặc định
//! không xoá gì cả.
//!
//! Ba mức an toàn, mỗi mức phải mở khoá riêng:
//! - **Lượt chạy chưa xong hoặc vừa động vào** → bỏ qua (có thể đang chạy ở
//!   tiến trình khác). Mở bằng `force`.
//! - **Worktree còn thay đổi chưa commit** → bỏ qua, vì xoá là mất hẳn. Mở
//!   bằng `force`.
//! - **Branch và event log** → giữ mặc định. Branch là toàn bộ việc agent đã
//!   làm; event log là thứ `runs`, `replay`, `ask` và lịch sử trên dashboard
//!   đọc. Mở bằng `branches` / `logs`.

use crate::event::{Event, EventKind};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Lượt chạy vừa động vào trong khoảng này coi như có thể đang chạy.
const VUA_CHAY: Duration = Duration::from_secs(120);

/// Một worktree còn sót của node.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub node: String,
    pub path: PathBuf,
    /// Còn thay đổi chưa commit — xoá là mất.
    pub dirty: bool,
}

/// Một branch còn sót của node.
#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    /// Có commit chưa nằm trong HEAD của repo — xoá là mất việc agent làm.
    pub unmerged: bool,
}

/// Rác của một lượt chạy.
#[derive(Debug, Clone)]
pub struct RunJunk {
    pub run: String,
    pub goal: String,
    /// Event log có `run_finished`.
    pub finished: bool,
    /// Event log vừa được ghi thêm — lượt chạy có thể còn sống.
    pub recent: bool,
    pub worktrees: Vec<Worktree>,
    pub branches: Vec<Branch>,
    pub log_dir: PathBuf,
    pub log_bytes: u64,
    pub worktree_bytes: u64,
}

impl RunJunk {
    /// Xoá lượt này có cần `force` không, và vì sao.
    pub fn risk(&self) -> Option<String> {
        if self.recent {
            return Some("event log vừa được ghi — có thể đang chạy".into());
        }
        if !self.finished {
            return Some("chưa có run_finished — dở dang hoặc đang chạy".into());
        }
        let dirty = self.worktrees.iter().filter(|w| w.dirty).count();
        if dirty > 0 {
            return Some(format!("{dirty} worktree còn thay đổi chưa commit"));
        }
        None
    }
}

/// Lựa chọn khi dọn. Mặc định (`Default`) là an toàn nhất: chỉ gỡ worktree
/// sạch của lượt đã xong, giữ nguyên branch và log.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Xoá luôn branch `al/<run>/*`.
    pub branches: bool,
    /// Xoá luôn thư mục log `.agentloom/runs/<run>`.
    pub logs: bool,
    /// Bỏ qua ba mức an toàn ở trên.
    pub force: bool,
}

/// Việc đã làm (hoặc sẽ làm) cho một lượt chạy.
#[derive(Debug, Clone, Default)]
pub struct Done {
    pub worktrees: usize,
    pub branches: usize,
    pub logs: usize,
    pub bytes: u64,
    /// Lượt bị bỏ qua kèm lý do.
    pub skipped: Vec<(String, String)>,
    pub errors: Vec<String>,
}

async fn git(dir: &Path, args: &[&str]) -> (bool, String) {
    match tokio::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .await
    {
        Ok(out) => {
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&out.stderr));
            (out.status.success(), s.trim().to_string())
        }
        Err(e) => (false, e.to_string()),
    }
}

/// Tổng dung lượng một cây thư mục. Lỗi đọc thì bỏ qua — đây là con số để
/// người dùng ước lượng, không phải để đối chiếu.
fn dung_luong(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut tong = 0;
    for e in rd.flatten() {
        match e.metadata() {
            Ok(m) if m.is_dir() => tong += dung_luong(&e.path()),
            Ok(m) => tong += m.len(),
            Err(_) => {}
        }
    }
    tong
}

/// Đọc `goal` và `run_finished` mà không fold cả log: log của một lượt chạy
/// dài có thể hàng chục nghìn dòng `agent_raw`, mà ở đây chỉ cần hai dòng.
fn doc_tom_tat(events: &Path) -> (String, bool, bool) {
    let Ok(text) = std::fs::read_to_string(events) else {
        return (String::new(), false, false);
    };
    let (mut goal, mut finished) = (String::new(), false);
    for line in text.lines() {
        let la_bat_dau = line.contains("\"run_started\"");
        if !la_bat_dau && !line.contains("\"run_finished\"") {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Event>(line) else {
            continue;
        };
        match ev.kind {
            EventKind::RunStarted { goal: g, .. } => goal = g,
            EventKind::RunFinished { .. } => finished = true,
            _ => {}
        }
    }
    let recent = std::fs::metadata(events)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|d| d < VUA_CHAY);
    (goal, finished, recent)
}

/// Nhìn xem còn gì của các lượt chạy cũ trong `root`. Không xoá gì.
/// Mới nhất trước — id lượt chạy bắt đầu bằng thời điểm tạo.
pub async fn scan(root: impl AsRef<Path>) -> Vec<RunJunk> {
    let root = root.as_ref();
    let la_repo = git(root, &["rev-parse", "--is-inside-work-tree"]).await;
    let repo_root = if la_repo.0 && la_repo.1 == "true" {
        let (ok, p) = git(root, &["rev-parse", "--show-toplevel"]).await;
        if ok {
            PathBuf::from(p)
        } else {
            root.to_path_buf()
        }
    } else {
        root.to_path_buf()
    };
    let la_repo = la_repo.0 && la_repo.1 == "true";

    // Branch chưa nằm trong HEAD: hỏi git một lần cho cả repo thay vì mỗi
    // branch một lần.
    let chua_merge: Vec<String> = if la_repo {
        git(
            &repo_root,
            &["branch", "--no-merged", "HEAD", "--format=%(refname:short)"],
        )
        .await
        .1
        .lines()
        .map(|l| l.trim().to_string())
        .collect()
    } else {
        Vec::new()
    };
    let moi_branch: Vec<String> = if la_repo {
        git(&repo_root, &["branch", "--format=%(refname:short)"])
            .await
            .1
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| l.starts_with("al/"))
            .collect()
    } else {
        Vec::new()
    };

    // `runs` (event log của `agentloom run`/`ask`/web) luôn nằm ngay dưới
    // `root` mà người dùng đưa — xem `main.rs`/`web.rs`. Nhưng `worktrees`
    // (và branch) luôn nằm ở gốc REPO GIT (`Worktrees::discover`), vì `git
    // worktree add` cần chạy từ đó. Khi `root` không trùng `repo_root` (ví dụ
    // `root` là một thư mục con chưa init git, nằm trong một repo git khác ở
    // ngoài) thì hai thứ này ở HAI chỗ khác nhau — gộp chung một `base` khiến
    // `scan` đọc nhầm `events.jsonl` của repo ngoài (hoặc không thấy gì) và
    // báo sai "chưa xong" cho lượt đã chạy hẳn hoi.
    let runs_base = root.join(".agentloom");
    let wt_base = repo_root.join(".agentloom");
    let mut ids: Vec<String> = Vec::new();
    for base in [&runs_base, &wt_base] {
        for sub in ["runs", "worktrees"] {
            let Ok(rd) = std::fs::read_dir(base.join(sub)) else {
                continue;
            };
            for e in rd.flatten() {
                let id = e.file_name().to_string_lossy().into_owned();
                if e.path().is_dir() && !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }
    // Branch còn sót của lượt chạy mà thư mục đã bị xoá tay vẫn phải hiện ra.
    for b in &moi_branch {
        if let Some(id) = b.split('/').nth(1)
            && !ids.contains(&id.to_string())
        {
            ids.push(id.to_string());
        }
    }
    ids.sort();
    ids.reverse();

    let mut out = Vec::new();
    for run in ids {
        let log_dir = runs_base.join("runs").join(&run);
        let (goal, finished, recent) = doc_tom_tat(&log_dir.join("events.jsonl"));
        let wt_dir = wt_base.join("worktrees").join(&run);
        let mut worktrees = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&wt_dir) {
            for e in rd.flatten() {
                if !e.path().is_dir() {
                    continue;
                }
                let path = e.path();
                let dirty = !git(&path, &["status", "--porcelain"]).await.1.is_empty();
                worktrees.push(Worktree {
                    node: e.file_name().to_string_lossy().into_owned(),
                    path,
                    dirty,
                });
            }
        }
        worktrees.sort_by(|a, b| a.node.cmp(&b.node));
        let branches = moi_branch
            .iter()
            .filter(|b| b.starts_with(&format!("al/{run}/")))
            .map(|b| Branch {
                name: b.clone(),
                unmerged: chua_merge.contains(b),
            })
            .collect();
        out.push(RunJunk {
            run,
            goal,
            finished,
            recent,
            worktrees,
            branches,
            log_bytes: dung_luong(&log_dir),
            log_dir,
            worktree_bytes: dung_luong(&wt_dir),
        });
    }
    out
}

/// Xoá thật. Chỉ động vào những lượt trong `targets` — gọi [`scan`] trước, lọc
/// rồi đưa vào đây, để giao diện quyết định hỏi hay không hỏi người dùng.
pub async fn clean(root: impl AsRef<Path>, targets: &[RunJunk], opts: &Options) -> Done {
    let root = root.as_ref();
    let (ok, top) = git(root, &["rev-parse", "--show-toplevel"]).await;
    let repo_root = if ok {
        PathBuf::from(top)
    } else {
        root.to_path_buf()
    };
    // Prune TRƯỚC vòng lặp, không phải sau: một worktree bị xoá tay (thư mục
    // mất nhưng git vẫn còn đăng ký) khiến `scan` báo "0 worktree" cho lượt
    // đó — vòng lặp bên dưới không còn gì để `worktree remove` — nhưng
    // `branch -D` vẫn bị git từ chối vì branch "còn dùng bởi worktree" đăng
    // ký mồ côi ấy. Prune ở cuối (như trước) dọn xong quá muộn: nó chỉ có
    // ích cho LẦN GỌI SAU, còn lần này branch xoá vẫn lỗi.
    let _ = git(&repo_root, &["worktree", "prune"]).await;
    let mut d = Done::default();
    for t in targets {
        if let Some(ly_do) = t.risk()
            && !opts.force
        {
            d.skipped.push((t.run.clone(), ly_do));
            continue;
        }
        for w in &t.worktrees {
            let bytes = dung_luong(&w.path);
            let p = w.path.to_string_lossy().into_owned();
            let (mut ok, mut err) = git(&repo_root, &["worktree", "remove", &p]).await;
            // File chưa track (build artifact…) làm `remove` từ chối. Với
            // worktree sạch thì --force chỉ xoá đúng những file đó.
            if !ok && (!w.dirty || opts.force) {
                (ok, err) = git(&repo_root, &["worktree", "remove", "--force", &p]).await;
            }
            if ok {
                d.worktrees += 1;
                d.bytes += bytes;
            } else {
                d.errors.push(format!("{}: {err}", w.path.display()));
            }
        }
        // Thư mục cha của run rỗng rồi thì bỏ luôn, đừng để lại vỏ.
        let _ = std::fs::remove_dir(repo_root.join(".agentloom").join("worktrees").join(&t.run));
        if opts.branches {
            for b in &t.branches {
                let (ok, err) = git(&repo_root, &["branch", "-D", &b.name]).await;
                if ok {
                    d.branches += 1;
                } else {
                    d.errors.push(format!("{}: {err}", b.name));
                }
            }
        }
        if opts.logs && t.log_dir.exists() {
            match std::fs::remove_dir_all(&t.log_dir) {
                Ok(()) => {
                    d.logs += 1;
                    d.bytes += t.log_bytes;
                }
                Err(e) => d.errors.push(format!("{}: {e}", t.log_dir.display())),
            }
        }
    }
    let _ = git(&repo_root, &["worktree", "prune"]).await;
    d
}

/// "1,2 MB" — số để người đọc ước lượng, không phải để tính toán.
pub fn kho(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b >= K * K * K {
        format!("{:.1} GB", b / (K * K * K))
    } else if b >= K * K {
        format!("{:.1} MB", b / (K * K))
    } else if b >= K {
        format!("{:.0} KB", b / K)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, EventLog};
    use crate::ids::RunId;

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
        s.push_str(&String::from_utf8_lossy(&out.stderr));
        s.trim().to_string()
    }

    /// Repo có sẵn một lượt chạy `run` với hai node: `a` (sạch, đã commit) và
    /// `b` (còn thay đổi chưa commit).
    fn dung_repo(tag: &str, run: &str, finished: bool) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ag-clean-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        sh(&d, &["init", "-q"]);
        sh(&d, &["config", "user.email", "a@b"]);
        sh(&d, &["config", "user.name", "t"]);
        std::fs::write(d.join("README.md"), "x").unwrap();
        sh(&d, &["add", "-A"]);
        sh(&d, &["commit", "-qm", "init"]);

        let log =
            EventLog::create(d.join(".agentloom/runs").join(run).join("events.jsonl")).unwrap();
        log.emit(
            None,
            EventKind::RunStarted {
                goal: "mục tiêu cũ".into(),
                run: RunId::generate(),
            },
        );
        if finished {
            log.emit(
                None,
                EventKind::RunFinished {
                    ok: true,
                    total_cost_usd: 0.0,
                },
            );
        }
        for node in ["a", "b"] {
            let p = d.join(".agentloom/worktrees").join(run).join(node);
            sh(
                &d,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &format!("al/{run}/{node}"),
                    &p.to_string_lossy(),
                    "HEAD",
                ],
            );
            std::fs::write(p.join("viec.txt"), node).unwrap();
            if node == "a" {
                sh(&p, &["add", "-A"]);
                sh(&p, &["commit", "-qm", "viec cua a"]);
            }
        }
        d
    }

    /// Lùi mtime của event log để nó không còn bị coi là "vừa chạy".
    fn lam_cu(root: &Path, run: &str) {
        let p = root.join(".agentloom/runs").join(run).join("events.jsonl");
        let cu = SystemTime::now() - Duration::from_secs(3600);
        let f = std::fs::File::options().append(true).open(&p).unwrap();
        f.set_modified(cu).unwrap();
    }

    #[tokio::test]
    async fn scan_thay_worktree_branch_va_thay_do_ban() {
        let run = "20260101-000000-aaaaaa";
        let d = dung_repo("scan", run, true);
        lam_cu(&d, run);
        let js = scan(&d).await;
        assert_eq!(js.len(), 1);
        let j = &js[0];
        assert_eq!(j.goal, "mục tiêu cũ");
        assert!(j.finished && !j.recent);
        assert_eq!(j.worktrees.len(), 2);
        assert!(!j.worktrees[0].dirty, "node a đã commit");
        assert!(j.worktrees[1].dirty, "node b còn file chưa commit");
        assert_eq!(j.branches.len(), 2);
        assert!(
            j.branches
                .iter()
                .any(|b| b.name.ends_with("/a") && b.unmerged),
            "branch a có commit riêng, chưa nằm trong HEAD: {:?}",
            j.branches
        );
        assert!(j.worktree_bytes > 0 && j.log_bytes > 0);
        std::fs::remove_dir_all(d).ok();
    }

    /// Bug thật: khi `root` KHÔNG phải gốc repo git (ví dụ một thư mục con
    /// chưa `git init`, nằm trong một repo git khác ở ngoài), `agentloom
    /// run`/`web` ghi `events.jsonl` ngay dưới `root` đó (xem `main.rs`,
    /// `web.rs`), còn `Worktrees::discover` (xem `workspace.rs`) lại đặt
    /// worktree ở GỐC REPO GIT — hai chỗ khác nhau. `scan` cũ gộp chung một
    /// `base = repo_root.join(".agentloom")` nên đọc nhầm (hoặc không thấy)
    /// event log thật, báo sai "chưa xong" cho một lượt đã `run_finished`
    /// hẳn hoi.
    #[tokio::test]
    async fn scan_dung_dung_event_log_khi_root_khong_phai_goc_repo() {
        let outer = std::env::temp_dir().join(format!("ag-clean-outer-{}", uuid::Uuid::new_v4()));
        let inner = outer.join("con-chua-git");
        std::fs::create_dir_all(&inner).unwrap();
        sh(&outer, &["init", "-q"]);
        sh(&outer, &["config", "user.email", "a@b"]);
        sh(&outer, &["config", "user.name", "t"]);
        std::fs::write(outer.join("README.md"), "x").unwrap();
        sh(&outer, &["add", "-A"]);
        sh(&outer, &["commit", "-qm", "init"]);

        let run = "20260101-000000-eeeeee";
        // Event log thật: dưới `inner` (đúng như `run --root inner` sẽ ghi).
        let log =
            EventLog::create(inner.join(".agentloom/runs").join(run).join("events.jsonl")).unwrap();
        log.emit(
            None,
            EventKind::RunStarted {
                goal: "việc trong thư mục con".into(),
                run: RunId::generate(),
            },
        );
        log.emit(
            None,
            EventKind::RunFinished {
                ok: true,
                total_cost_usd: 0.0,
            },
        );
        lam_cu(&inner, run);
        // Worktree thật: ở GỐC repo git `outer` (đúng như `Worktrees::discover`).
        let wt = outer.join(".agentloom/worktrees").join(run).join("a");
        sh(
            &outer,
            &[
                "worktree",
                "add",
                "-b",
                &format!("al/{run}/a"),
                &wt.to_string_lossy(),
                "HEAD",
            ],
        );

        let js = scan(&inner).await;
        assert_eq!(js.len(), 1, "{js:?}");
        let j = &js[0];
        assert_eq!(j.goal, "việc trong thư mục con", "{j:?}");
        assert!(
            j.finished,
            "log đã có run_finished, không được coi là dở dang: {j:?}"
        );
        assert!(
            j.log_bytes > 0,
            "phải đọc đúng event log dưới root, không phải dưới repo_root"
        );
        assert_eq!(
            j.worktrees.len(),
            1,
            "worktree ở gốc repo vẫn phải thấy: {j:?}"
        );

        std::fs::remove_dir_all(&outer).ok();
    }

    #[tokio::test]
    async fn mac_dinh_giu_branch_va_log_chi_go_worktree() {
        let run = "20260101-000000-bbbbbb";
        let d = dung_repo("macdinh", run, true);
        lam_cu(&d, run);
        let js = scan(&d).await;
        // Worktree bẩn khiến cả lượt bị chặn — đúng như thiết kế.
        let done = clean(&d, &js, &Options::default()).await;
        assert_eq!(done.worktrees, 0);
        assert_eq!(done.skipped.len(), 1, "{done:?}");
        assert!(done.skipped[0].1.contains("chưa commit"));

        // Commit nốt node b rồi dọn lại: worktree đi, branch và log ở lại.
        let b = d.join(".agentloom/worktrees").join(run).join("b");
        sh(&b, &["add", "-A"]);
        sh(&b, &["commit", "-qm", "xong"]);
        let js = scan(&d).await;
        let done = clean(&d, &js, &Options::default()).await;
        assert_eq!(done.worktrees, 2, "{done:?}");
        assert_eq!((done.branches, done.logs), (0, 0));
        assert!(!d.join(".agentloom/worktrees").join(run).exists());
        assert!(d.join(".agentloom/runs").join(run).exists());
        assert!(sh(&d, &["branch", "--list", "al/*"]).contains(&format!("al/{run}/a")));
        std::fs::remove_dir_all(d).ok();
    }

    #[tokio::test]
    async fn force_xoa_ca_worktree_ban_branch_va_log() {
        let run = "20260101-000000-cccccc";
        let d = dung_repo("force", run, false); // chưa có run_finished
        lam_cu(&d, run);
        let js = scan(&d).await;
        assert!(!js[0].finished);
        let done = clean(
            &d,
            &js,
            &Options {
                branches: true,
                logs: true,
                force: true,
            },
        )
        .await;
        assert_eq!((done.worktrees, done.branches, done.logs), (2, 2, 1));
        assert!(done.errors.is_empty(), "{:?}", done.errors);
        assert!(done.bytes > 0);
        assert!(!d.join(".agentloom/runs").join(run).exists());
        assert_eq!(sh(&d, &["branch", "--list", "al/*"]), "");
        assert_eq!(sh(&d, &["worktree", "list"]).lines().count(), 1);
        std::fs::remove_dir_all(d).ok();
    }

    /// Bug thật: worktree bị xoá TAY (thư mục mất, nhưng git vẫn còn đăng ký
    /// — `git worktree list` báo "prunable"). `scan` đúng đắn báo "0
    /// worktree" cho lượt này (thư mục không còn), nên vòng lặp `worktree
    /// remove` trong `clean` không có gì để làm. Nhưng git vẫn từ chối
    /// `branch -D` vì branch "còn dùng bởi worktree" đăng ký mồ côi đó —
    /// prune phải chạy TRƯỚC khi xoá branch trong CÙNG một lần gọi `clean`,
    /// không phải sau (prune ở cuối chỉ có ích cho lần gọi kế tiếp).
    #[tokio::test]
    async fn worktree_mo_coi_khong_chan_duoc_xoa_branch() {
        let run = "20260101-000000-ffffff";
        let d = dung_repo("mocoi", run, true);
        lam_cu(&d, run);
        // Xoá tay thư mục worktree của node "a" (đã commit, sạch) — mô phỏng
        // người dùng `rm -rf` thay vì `git worktree remove`.
        std::fs::remove_dir_all(d.join(".agentloom/worktrees").join(run).join("a")).unwrap();
        let js = scan(&d).await;
        assert_eq!(
            js[0].worktrees.len(),
            1,
            "chỉ còn thấy node b: {:?}",
            js[0].worktrees
        );

        let done = clean(
            &d,
            &js,
            &Options {
                branches: true,
                logs: false,
                force: true,
            },
        )
        .await;
        assert!(done.errors.is_empty(), "branch -D không được lỗi: {done:?}");
        assert_eq!(done.branches, 2, "{done:?}");
        assert_eq!(sh(&d, &["branch", "--list", "al/*"]), "");
        std::fs::remove_dir_all(d).ok();
    }

    #[tokio::test]
    async fn luot_vua_chay_khong_bi_dong_vao() {
        let run = "20260101-000000-dddddd";
        let d = dung_repo("moi", run, true); // log vừa ghi xong
        let js = scan(&d).await;
        assert!(js[0].recent);
        let done = clean(&d, &js, &Options::default()).await;
        assert_eq!(done.worktrees, 0);
        assert!(done.skipped[0].1.contains("đang chạy"));
        std::fs::remove_dir_all(d).ok();
    }
}
