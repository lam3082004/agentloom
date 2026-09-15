//! Orchestrator: lập lịch graph động, chạy agent song song, áp mutation.
//!
//! Thiết kế cố ý giữ graph trong **một** task duy nhất. Các adapter chạy
//! trong task con nhưng không đụng vào graph — chúng chỉ trả về outcome và
//! ghi event. Nhờ vậy không có Mutex nào quanh graph, và thứ tự áp mutation
//! là tất định.

use crate::agent::{AgentOutcome, AgentRequest, adapter_for};
use crate::config::{Limits, Plan};
use crate::event::{EventKind, EventLog, Origin};
use crate::graph::{Graph, Isolate, NodeState};
use crate::harness::{self, HarnessStore, MUTATION_FILE, Mutation};
use crate::ids::{NodeId, RunId};
use crate::workspace::Worktrees;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::task::JoinSet;

pub struct RunSummary {
    pub run: RunId,
    pub ok: bool,
    pub total_cost_usd: f64,
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub events_path: PathBuf,
}

/// Hướng dẫn protocol nhét vào task của mọi agent.
///
/// Đây chính là context engineering của tầng meta: agent con không biết gì
/// về hệ này, nên harness phải *dạy* nó cách đề nghị sửa graph.
fn protocol_block(store: &HarnessStore) -> String {
    let mut s = String::from(
        "\n\n---\n\
         Bạn đang chạy như một node trong một graph nhiều agent.\n\
         \n\
         KÊNH DUY NHẤT để nói chuyện với hệ điều phối là nối thêm dòng JSON vào\n\
         `.agentloom/mutations.jsonl` trong thư mục làm việc. Mỗi dòng một lệnh:\n\
         \n\
         mkdir -p .agentloom && cat >> .agentloom/mutations.jsonl <<'JSONL'\n\
         {\"op\":\"spawn\",\"id\":\"<ten-node>\",\"agent\":\"<claude hoac codex>\",\"task\":\"<viec>\",\"verify\":\"<lenh shell kiem chung>\",\"model\":\"<model, tuy chon>\"}\n\
         {\"op\":\"write_skill\",\"name\":\"<ten>\",\"body\":\"<quy trinh tai su dung>\"}\n\
         {\"op\":\"write_memory\",\"key\":\"<ten>\",\"value\":\"<noi dung>\"}\n\
         JSONL\n\
         \n\
         Thay phần trong <> bằng giá trị thật; copy nguyên mẫu sẽ bị từ chối.\n\
         Node mới tự động phụ thuộc node này. Đừng spawn nếu tự làm nhanh hơn.\n\
         Luôn kèm `verify`: lệnh shell thoát 0 khi và chỉ khi việc của node con\n\
         thật sự xong (ví dụ `python3 check.py`, `cargo test -q`). Không có nó thì\n\
         hệ điều phối chỉ còn tin lời node con.\n\
         `model`: BẠN quyết định model cho từng node con. Node con KHÔNG kế thừa\n\
         model của bạn — bỏ trống thì nó chạy model mặc định của CLI agent. Nếu\n\
         việc được giao đã chỉ định model cho node con thì PHẢI dùng đúng model\n\
         đó; không thì tự chọn theo độ khó: việc đơn giản, lặp lại → model nhẹ\n\
         (claude: `sonnet`), việc khó, cần thiết kế hay suy luận dài → model mạnh\n\
         (claude: `opus`). Với codex, chỉ ghi model khi biết chắc tên model hợp lệ.\n\
         \n\
         QUAN TRỌNG: khi được yêu cầu ghi lại một quy trình để tái sử dụng, PHẢI\n\
         dùng `write_skill` ở trên. ĐỪNG dùng cơ chế skill/memory riêng của bạn\n\
         (`.claude/skills/`, `AGENTS.md`, `CLAUDE.md`, bộ nhớ nội bộ...) — hệ điều\n\
         phối không đọc những chỗ đó, và công sức của bạn sẽ mất khi node kết thúc.\n",
    );
    let skills = store.load_skills();
    if !skills.is_empty() {
        s.push_str("\nQuy trình đã tích luỹ từ các lần chạy trước:\n");
        for (name, body) in skills.iter().take(8) {
            s.push_str(&format!("\n## {name}\n{body}\n"));
        }
    }
    s
}

/// Báo đụng độ merge cho agent qua kênh operator — cùng lý do `protocol_block`
/// không đi qua task: đây là chỉ dẫn của hệ điều phối, không phải của người
/// dùng, nhét vào task sẽ bị agent có ý thức bảo mật coi là prompt injection.
///
/// Không có dòng này thì agent chạy trên cây THIẾU việc của dep mà không biết
/// — merge đụng độ trước đây chỉ ghi Note vào event log, nơi agent không đọc.
fn conflict_block(branches: &[String]) -> String {
    format!(
        "\n\n---\n\
         CẢNH BÁO TỪ HỆ ĐIỀU PHỐI: merge nhánh của (các) node phụ thuộc vào\n\
         worktree này bị ĐỤNG ĐỘ và đã bị bỏ qua (git merge --abort). Cây bạn\n\
         đang thấy CÓ THỂ THIẾU việc của những nhánh sau — tự kiểm tra bằng\n\
         `git log`/`git diff` với branch tương ứng và merge/áp lại thủ công nếu\n\
         cần thiết cho việc của bạn:\n{}\n",
        branches
            .iter()
            .map(|b| format!("  - {b}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

pub struct Runner {
    pub graph: Graph,
    log: EventLog,
    limits: Limits,
    store: HarnessStore,
    worktrees: Worktrees,
    run: RunId,
    /// Số dòng đã đọc của từng FILE mutation, để tail không đọc lại. Khoá theo
    /// đường dẫn chứ không theo node: ở chế độ `shared` mọi node dùng chung một
    /// file, khoá theo node thì node chạy sau đọc lại từ dòng 0 và áp lại
    /// mutation của node trước.
    cursors: HashMap<PathBuf, usize>,
    workspaces: HashMap<NodeId, PathBuf>,
    /// Nguồn tín hiệu huỷ, nhân bản (`subscribe`) cho mỗi request gửi tới
    /// adapter. Một `watch` chứ không phải `Notify`: node khởi động SAU khi
    /// đã huỷ vẫn phải đọc được giá trị `true` ngay, không chỉ node đang chạy
    /// lúc tín hiệu phát ra.
    cancel_tx: tokio::sync::watch::Sender<bool>,
    /// Giữ sống MỘT receiver suốt đời Runner — không dùng tới, chỉ để không
    /// bao giờ tụt về 0. `watch::Sender::send` khi KHÔNG còn receiver nào trả
    /// lỗi và BỎ QUA giá trị gửi, không cập nhật gì cả (khác `send_replace`).
    /// Giữa hai node (không có agent nào đang subscribe `req.cancel`), hoặc
    /// lúc verifier đang chạy (verifier không subscribe), receiver count có
    /// thể về 0 — canceller().send(true) từ CLI khi đó câm lặng mất tác dụng,
    /// `cancelled` không bao giờ thành `true`. Bug thật: huỷ giữa lúc verifier
    /// đang chạy hoặc giữa hai node không hề dừng được gì.
    _cancel_guard: tokio::sync::watch::Receiver<bool>,
}

impl Runner {
    /// `run` đến từ ngoài để thư mục event log, tên worktree và tên branch
    /// cùng mang một id — không có nó, log của một lượt chạy không lần ra
    /// được branch mà agent đã để lại.
    pub async fn new(
        root: impl Into<PathBuf>,
        limits: Limits,
        log: EventLog,
        run: RunId,
    ) -> anyhow::Result<Self> {
        let root: PathBuf = root.into();
        // max_parallel = 0 làm vòng lập lịch không bao giờ nạp được node nào
        // và quay vô hạn; một trần 0 không có nghĩa gì nên coi như 1.
        let limits = Limits {
            max_parallel: limits.max_parallel.max(1),
            ..limits
        };
        let worktrees = Worktrees::discover(&root, run.as_str()).await;
        let store = HarnessStore::new(worktrees.repo_root().join(".agentloom"));
        let (cancel_tx, cancel_guard) = tokio::sync::watch::channel(false);
        Ok(Self {
            graph: Graph::new(limits.max_nodes),
            log,
            limits,
            store,
            worktrees,
            run,
            cursors: HashMap::new(),
            workspaces: HashMap::new(),
            cancel_tx,
            _cancel_guard: cancel_guard,
        })
    }

    pub fn run_id(&self) -> &RunId {
        &self.run
    }

    /// Tay cầm để mã gọi từ ngoài (CLI) huỷ toàn bộ agent con đang chạy.
    /// `Sender` là `Clone`, gọi `.send(true)` từ đâu cũng được — orchestrator
    /// không cần biết TUI, `--plain` hay `--web` gọi nó theo cách nào.
    pub fn canceller(&self) -> tokio::sync::watch::Sender<bool> {
        self.cancel_tx.clone()
    }

    pub async fn execute(mut self, plan: Plan) -> anyhow::Result<RunSummary> {
        self.log.emit(
            None,
            EventKind::RunStarted {
                goal: plan.goal.clone(),
                run: self.run.clone(),
            },
        );
        if !self.worktrees.enabled() {
            self.log.emit(
                None,
                EventKind::Note {
                    text:
                        "không phải git repo — cô lập worktree bị tắt, mọi node dùng chung thư mục"
                            .into(),
                },
            );
        }

        // `Graph::add` đòi dep phải có sẵn, nhưng plan không bắt người viết sắp
        // xếp topo — nên nạp theo nhiều vòng, mỗi vòng thêm node đã đủ dep.
        let mut pending = plan.nodes;
        let mut rejected = 0usize;
        while !pending.is_empty() {
            let mut left = Vec::new();
            let mut added = false;
            for spec in pending {
                if spec
                    .deps
                    .iter()
                    .any(|d| self.graph.get(d).is_none() && *d != spec.id)
                {
                    left.push(spec);
                    continue;
                }
                let id = spec.id.clone();
                let (title, agent, deps, model) = (
                    spec.title.clone(),
                    spec.agent.clone(),
                    spec.deps.clone(),
                    spec.model.clone(),
                );
                match self.graph.add(spec) {
                    Ok(()) => {
                        added = true;
                        self.log.emit(
                            Some(id),
                            EventKind::NodeAdded {
                                title,
                                agent,
                                deps,
                                by: Origin::Plan,
                                model,
                            },
                        );
                    }
                    Err(e) => {
                        rejected += 1;
                        self.log.emit(
                            Some(id.clone()),
                            EventKind::Mutation {
                                op: "plan_add".into(),
                                target: id.to_string(),
                                accepted: false,
                                reason: e.to_string(),
                            },
                        );
                    }
                }
            }
            if !added {
                // Còn node mà không thêm được node nào nữa: dep ma hoặc chu
                // trình. Ghi lý do rồi dừng, đừng im lặng bỏ qua.
                rejected += left.len();
                for spec in left {
                    self.log.emit(
                        Some(spec.id.clone()),
                        EventKind::Mutation {
                            op: "plan_add".into(),
                            target: spec.id.to_string(),
                            accepted: false,
                            reason: "dep không tồn tại hoặc tạo chu trình".into(),
                        },
                    );
                }
                break;
            }
            pending = left;
        }

        let mut set: JoinSet<(NodeId, anyhow::Result<AgentOutcome>)> = JoinSet::new();
        let mut running: Vec<NodeId> = Vec::new();
        let mut tick = tokio::time::interval(Duration::from_millis(400));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut budget_hit = false;

        let mut cancel_noted = false;

        loop {
            let doi = self.graph.refresh_ready();
            // Graph đổi trạng thái bên trong; không phát event thì TUI, web,
            // replay và `runs` (đều fold từ event log) thấy node này "đang chờ"
            // mãi mãi dù lượt chạy đã xong.
            for (id, dep) in doi.skipped {
                self.log.emit(
                    Some(id.clone()),
                    EventKind::NodeState {
                        state: "skipped".into(),
                    },
                );
                self.log.emit(
                    Some(id),
                    EventKind::Note {
                        text: format!("bỏ qua vì agent trước '{dep}' không xong"),
                    },
                );
            }
            // Đọc một lần mỗi vòng: `cancel_tx` có thể đổi bất cứ lúc nào từ
            // ngoài (CLI), nhưng vòng nạp node bên dưới chạy đồng bộ nên phải
            // chốt giá trị trước khi quyết định có nạp thêm hay không.
            let cancelled = *self.cancel_tx.borrow();
            if cancelled && !cancel_noted {
                cancel_noted = true;
                self.log.emit(
                    None,
                    EventKind::Note {
                        text: "bị huỷ — dừng nạp node mới, giết agent đang chạy".into(),
                    },
                );
            }

            // Nạp node mới vào chỗ trống, trong giới hạn song song và ngân sách.
            while !cancelled && running.len() < self.limits.max_parallel {
                if self.graph.total_cost() >= self.limits.max_total_usd {
                    if !budget_hit {
                        budget_hit = true;
                        self.log.emit(
                            None,
                            EventKind::Note {
                                text: format!(
                                    "chạm trần ${:.2} — không nạp thêm node",
                                    self.limits.max_total_usd
                                ),
                            },
                        );
                    }
                    break;
                }
                let Some(id) = self.graph.ready().into_iter().next() else {
                    break;
                };
                match self.start(&id).await {
                    Ok(req) => {
                        let agent = self
                            .graph
                            .get(&id)
                            .map(|n| n.spec.agent.clone())
                            .unwrap_or_else(|| "fake".into());
                        let log = self.log.clone();
                        let idc = id.clone();
                        set.spawn(async move {
                            let Some(ad) = adapter_for(&agent) else {
                                return (
                                    idc,
                                    Err(anyhow::anyhow!("không có adapter cho '{agent}'")),
                                );
                            };
                            let r = ad.run(req, &log).await;
                            (idc, r)
                        });
                        running.push(id);
                    }
                    Err(e) => {
                        self.finish(
                            &id,
                            false,
                            0.0,
                            &format!("không chuẩn bị được workspace: {e}"),
                            (0, 0),
                        )
                        .await;
                    }
                }
            }

            if running.is_empty() {
                // Không còn gì chạy và không còn gì mở khoá được -> xong.
                if self.graph.ready().is_empty() {
                    break;
                }
                if budget_hit || cancelled {
                    break;
                }
                continue;
            }

            tokio::select! {
                Some(joined) = set.join_next() => {
                    let (id, res) = joined?;
                    running.retain(|x| x != &id);
                    // Đọc nốt mutation agent ghi ngay trước khi thoát; tiến
                    // trình đã kết thúc nên file không còn dòng viết dở.
                    self.drain_mutations(&id, false).await;
                    match res {
                        Ok(o) => {
                            if let Some(n) = self.graph.node_mut(&id) { n.session = o.session.clone(); }
                            if o.ok && self.worktrees.enabled() && self.worktrees.has_changes(&id).await {
                                let _ = self.worktrees
                                    .commit_all(&id, &format!("agentloom: {id}"))
                                    .await;
                            }
                            let mut ok = o.ok;
                            let mut summary = o.summary.clone();
                            if ok {
                                if let Some((pass, msg)) = self.verify(&id).await {
                                    self.log.emit(
                                        Some(id.clone()),
                                        EventKind::Note {
                                            text: format!(
                                                "verifier {}: {}",
                                                if pass { "xanh" } else { "ĐỎ" },
                                                msg
                                            ),
                                        },
                                    );
                                    if !pass {
                                        ok = false;
                                        summary =
                                            format!("verifier đỏ — {msg}");
                                    }
                                }
                            }
                            self.finish(&id, ok, o.cost_usd, &summary, (o.tokens_in, o.tokens_out))
                                .await;
                        }
                        Err(e) => self.finish(&id, false, 0.0, &e.to_string(), (0, 0)).await,
                    }
                }
                _ = tick.tick() => {
                    // Fan-out động: agent còn đang chạy vẫn spawn được node mới.
                    for id in running.clone() {
                        self.drain_mutations(&id, true).await;
                    }
                }
            }
        }

        let done = self
            .graph
            .iter()
            .filter(|n| n.state == NodeState::Done)
            .count();
        let failed = self
            .graph
            .iter()
            .filter(|n| n.state == NodeState::Failed)
            .count();
        let skipped = self
            .graph
            .iter()
            .filter(|n| n.state == NodeState::Skipped)
            .count();
        let total = self.graph.total_cost();
        // Node trong plan bị từ chối cũng là lỗi: báo "OK" trong khi một phần
        // plan không hề chạy là đúng kiểu sai nguy hiểm nhất.
        let ok = failed == 0 && !budget_hit && rejected == 0 && !*self.cancel_tx.borrow();
        self.log.emit(
            None,
            EventKind::RunFinished {
                ok,
                total_cost_usd: total,
            },
        );
        Ok(RunSummary {
            run: self.run.clone(),
            ok,
            total_cost_usd: total,
            done,
            failed,
            skipped,
            events_path: self.log.path().to_path_buf(),
        })
    }

    /// Chuẩn bị workspace + dựng request. Join sẽ merge branch của các dep vào đây.
    async fn start(&mut self, id: &NodeId) -> anyhow::Result<AgentRequest> {
        let node = self
            .graph
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("không có node {id}"))?;
        let (task, deps, model, isolate, session) = (
            node.spec.task.clone(),
            node.spec.deps.clone(),
            node.spec.model.clone(),
            node.spec.isolate,
            node.session.clone(),
        );

        let cwd = match isolate {
            Isolate::Worktree => self.worktrees.create(id).await?,
            Isolate::Shared => self.worktrees.repo_root().to_path_buf(),
        };
        self.log.emit(
            Some(id.clone()),
            EventKind::Workspace {
                action: if isolate == Isolate::Worktree {
                    "worktree"
                } else {
                    "shared"
                }
                .into(),
                path: cwd.to_string_lossy().into_owned(),
            },
        );

        // Gom công việc của MỌI dep vào cây này trước khi chạy — kể cả khi
        // chỉ có một dep. Worktree mới luôn tạo từ HEAD, nên bỏ bước merge là
        // node sau nhìn vào cây trống và mọi chuỗi "làm rồi review" thành vô
        // nghĩa.
        let mut conflicts = Vec::new();
        if !deps.is_empty() && isolate == Isolate::Worktree {
            conflicts = self
                .worktrees
                .merge_into(id, &deps)
                .await
                .unwrap_or_default();
            if !conflicts.is_empty() {
                self.log.emit(
                    Some(id.clone()),
                    EventKind::Note {
                        text: format!(
                            "merge đụng độ, agent phải tự xử lý: {}",
                            conflicts.join(", ")
                        ),
                    },
                );
            }
        }

        self.workspaces.insert(id.clone(), cwd.clone());
        self.graph.set_state(id, NodeState::Running)?;
        self.log.emit(
            Some(id.clone()),
            EventKind::NodeState {
                state: "running".into(),
            },
        );

        // Đụng độ đi qua kênh operator giống `protocol_block` — agent phải
        // biết cây mình đang chạy CÓ THỂ thiếu việc của dep, chứ không chỉ có
        // Note nằm im trong event log mà agent không đọc.
        let mut protocol = protocol_block(&self.store);
        if !conflicts.is_empty() {
            protocol.push_str(&conflict_block(&conflicts));
        }

        Ok(AgentRequest {
            node: id.clone(),
            task,
            protocol,
            cwd,
            session,
            model,
            permission_mode: self.limits.permission_mode.clone(),
            timeout: self.limits.node_timeout,
            cancel: self.cancel_tx.subscribe(),
        })
    }

    /// Verifier có quyền phủ quyết agent: agent nói xong mà lệnh này đỏ thì
    /// node vẫn HỎNG. Không có verifier thì ta chỉ đang tin lời model.
    async fn verify(&self, id: &NodeId) -> Option<(bool, String)> {
        let cmd = self.graph.get(id)?.spec.verify.clone()?;
        let cwd = self.workspaces.get(id)?.clone();
        // Không chạy được verifier thì coi như ĐỎ: im lặng cho qua đúng bằng
        // việc không có verifier, mà node này lại có khai báo verify.
        let mut command = crate::agent::shell(&cmd);
        command
            .current_dir(&cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // Nhóm tiến trình riêng: lệnh verify thường đẻ tiến trình con (shell
        // gọi tiếp lệnh khác), giết mỗi tiến trình trực tiếp sẽ để lại đám
        // con mồ côi chạy mãi.
        crate::agent::own_process_group(&mut command);
        let child = command.spawn();
        let child = match child {
            Ok(c) => c,
            Err(e) => return Some((false, format!("không chạy được verify: {e}"))),
        };
        let pgid = child.id();
        let kill_pgid = || async move {
            if let Some(p) = pgid {
                crate::agent::kill_process_group(p).await;
            }
        };
        // Verifier cũng phải có trần thời gian: một lệnh treo (test chờ nhập,
        // server không thoát) từng làm đứng cả orchestrator, vô thời hạn.
        //
        // Verifier PHẢI nghe tín hiệu huỷ như agent — trước đây nó không, nên
        // q/Ctrl-C giữa lúc verifier đang chạy (ví dụ `cargo test` lâu) bị
        // orchestrator lờ đi tới hết `node_timeout` (mặc định 30 phút) thay
        // vì dừng ngay.
        let mut cancel_rx = self.cancel_tx.subscribe();
        let out = tokio::select! {
            r = child.wait_with_output() => match r {
                Ok(o) => o,
                Err(e) => return Some((false, format!("verify lỗi: {e}"))),
            },
            _ = tokio::time::sleep(self.limits.node_timeout) => {
                kill_pgid().await;
                return Some((
                    false,
                    format!("verifier hết giờ sau {:?}", self.limits.node_timeout),
                ));
            }
            _ = crate::agent::wait_for_cancel(&mut cancel_rx) => {
                kill_pgid().await;
                return Some((false, "verifier bị huỷ theo yêu cầu người dùng".into()));
            }
        };
        let mut msg = String::from_utf8_lossy(&out.stdout).into_owned();
        msg.push_str(&String::from_utf8_lossy(&out.stderr));
        Some((out.status.success(), msg.trim().chars().take(400).collect()))
    }

    async fn finish(
        &mut self,
        id: &NodeId,
        ok: bool,
        cost: f64,
        summary: &str,
        tokens: (u64, u64),
    ) {
        let mut session = None;
        if let Some(n) = self.graph.node_mut(id) {
            n.cost_usd = cost;
            n.summary = summary.to_string();
            session = n.session.clone();
        }
        let _ = self.graph.set_state(
            id,
            if ok {
                NodeState::Done
            } else {
                NodeState::Failed
            },
        );
        let (ti, to) = tokens;
        self.log.emit(
            Some(id.clone()),
            EventKind::NodeFinished {
                ok,
                cost_usd: cost,
                tokens_in: ti,
                tokens_out: to,
                summary: summary.chars().take(400).collect(),
                session,
            },
        );
        self.log.emit(
            Some(id.clone()),
            EventKind::NodeState {
                state: if ok { "done" } else { "failed" }.into(),
            },
        );
    }

    /// Đọc mutation mới của một node và áp — validate trước, log cả khi từ chối.
    async fn drain_mutations(&mut self, id: &NodeId, still_running: bool) {
        let Some(ws) = self.workspaces.get(id).cloned() else {
            return;
        };
        let path = ws.join(MUTATION_FILE);
        let cursor = self.cursors.get(&path).copied().unwrap_or(0);
        let (muts, new_cursor) = harness::read_new(&path, cursor, still_running);
        if new_cursor != cursor {
            self.cursors.insert(path.clone(), new_cursor);
        }
        for m in muts {
            let (op, target) = (m.op().to_string(), m.target());
            let is_spawn = matches!(m, Mutation::Spawn { .. });
            let spawn_info = if let Mutation::Spawn { agent, title, .. } = &m {
                Some((
                    agent.clone(),
                    title.clone().unwrap_or_else(|| target.clone()),
                ))
            } else {
                None
            };
            match harness::apply(&m, id, &mut self.graph, &self.store) {
                Ok(()) => {
                    self.log.emit(
                        Some(id.clone()),
                        EventKind::Mutation {
                            op: op.clone(),
                            target: target.clone(),
                            accepted: true,
                            reason: String::new(),
                        },
                    );
                    if is_spawn {
                        if let (Some((agent, title)), Ok(nid)) =
                            (spawn_info, NodeId::new(target.clone()))
                        {
                            // Dep lấy từ graph: `apply` đã gộp cha với `after`,
                            // ghi lại mỗi cha là hiển thị sai hình dạng graph.
                            let deps = self
                                .graph
                                .get(&nid)
                                .map(|n| n.spec.deps.clone())
                                .unwrap_or_else(|| vec![id.clone()]);
                            let model = self.graph.get(&nid).and_then(|n| n.spec.model.clone());
                            let unverified = self
                                .graph
                                .get(&nid)
                                .is_some_and(|n| n.spec.verify.is_none());
                            self.log.emit(
                                Some(nid.clone()),
                                EventKind::NodeAdded {
                                    title,
                                    agent,
                                    deps,
                                    by: Origin::Agent,
                                    model,
                                },
                            );
                            // Sau NodeAdded: fold bỏ qua dòng log của node chưa
                            // tồn tại, đặt trước thì cảnh báo vô hình trên UI.
                            if unverified {
                                self.log.emit(
                                    Some(nid),
                                    EventKind::Note {
                                        text: "node do agent spawn không có verify — chỉ tin lời agent"
                                            .into(),
                                    },
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    self.log.emit(
                        Some(id.clone()),
                        EventKind::Mutation {
                            op,
                            target,
                            accepted: false,
                            reason: e.to_string(),
                        },
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (HarnessStore, PathBuf) {
        let d = std::env::temp_dir().join(format!("ag-pb-{}", uuid::Uuid::new_v4()));
        (HarnessStore::new(&d), d)
    }

    #[test]
    fn quy_trinh_da_ghi_duoc_nap_lai_o_lan_chay_sau() {
        let (s, d) = store();
        // Chưa có gì thì không được bịa ra mục "đã tích luỹ".
        assert!(!protocol_block(&s).contains("tích luỹ"));

        s.write_skill("kiem-tra-nhanh", "Buoc 1: git status")
            .unwrap();
        let p = protocol_block(&s);
        assert!(
            p.contains("kiem-tra-nhanh"),
            "tên quy trình phải vào context"
        );
        assert!(
            p.contains("Buoc 1: git status"),
            "nội dung phải vào context"
        );
        std::fs::remove_dir_all(d).ok();
    }

    /// Agent thật đã từng bỏ qua `write_skill` và ghi vào `.claude/skills/` của
    /// riêng nó — công sức mất sạch khi node kết thúc. Protocol phải nói thẳng
    /// điều đó, nếu không prior của agent sẽ thắng.
    #[test]
    fn protocol_chan_agent_dung_co_che_skill_rieng() {
        let (s, d) = store();
        let p = protocol_block(&s);
        assert!(
            p.contains(".claude/skills/"),
            "phải gọi đích danh chỗ agent hay ghi nhầm"
        );
        assert!(p.contains("AGENTS.md"));
        assert!(
            p.contains("\"verify\""),
            "phải dạy agent kèm verify khi spawn"
        );
        // Chạy thật: được dặn cho agent con dùng sonnet, agent chính vẫn spawn
        // không kèm model vì mẫu protocol không hề nhắc tới trường này.
        assert!(
            p.contains("\"model\""),
            "phải dạy agent chọn model cho node con"
        );
        assert!(
            p.contains("KHÔNG kế thừa"),
            "phải nói rõ node con không kế thừa model — agent cha tự quyết"
        );
        assert!(
            p.contains(".agentloom/mutations.jsonl"),
            "phải chỉ rõ kênh duy nhất được đọc"
        );
        std::fs::remove_dir_all(d).ok();
    }
}
