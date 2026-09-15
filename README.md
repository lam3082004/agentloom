# agentgraph

Điều phối nhiều coding agent (claude code, codex) theo **graph động** trong một
**harness động**, viết bằng Rust. Xem live bằng TUI.

Hệ này không tự gọi model. Mỗi node là một tiến trình agent có sẵn chạy
headless; việc của agentgraph là **lập lịch, cô lập, kiểm chứng** và cho phép
graph tự mọc thêm trong lúc chạy.

```
crates/core/            lõi, không biết gì về UI
  ids.rs                  NodeId/RunId — chặn id do agent đặt phá đường dẫn
  event.rs                event log JSONL append-only + broadcast
  graph.rs                DAG động: dep, chu trình, fan-out, join, trần node
  agent/                  adapter: claude · codex · fake (để test offline)
  harness.rs              protocol mutation: spawn / write_skill / write_memory
  workspace.rs            git worktree mỗi node, merge khi join
  run.rs                  scheduler: song song, ngân sách, verifier
  config.rs               plan TOML + mọi trần chi phí

crates/core/src/view.rs   fold event -> trạng thái hiển thị (dùng chung mọi mặt)

crates/cli/             binary `agentgraph`
  tui.rs                  ratatui: cây graph | log node | thanh chi phí
  web.rs                  axum + SSE, đẩy Patch đã fold sẵn
  assets/index.html       trang web, không thư viện ngoài
  main.rs                 run · replay · doctor · ask · runs
```

## Chạy

```bash
cargo build --release
./target/release/agentgraph doctor          # kiểm tra claude/codex/git

# một node
agentgraph run --goal "Sửa test đang đỏ" --agent claude

# một plan
agentgraph run plan.toml --parallel 3 --budget 5.0

# xem lại lượt cũ
agentgraph replay .agentgraph/runs/<id>/events.jsonl

# giao diện web thay vì TUI
agentgraph run plan.toml --web --port 7878
agentgraph replay .agentgraph/runs/<id>/events.jsonl --web

# liệt kê các lượt chạy, mới nhất trước
agentgraph runs

# hỏi lại agent của một node cũ — resume đúng session, đúng worktree
agentgraph ask .agentgraph/runs/<id>/events.jsonl <node-id> "câu hỏi"
```

```toml
# plan.toml
goal = "thêm auth"

[[node]]
id = "scan"
title = "đọc kiến trúc"
agent = "claude"
task = "Đọc src/ và mô tả luồng request."

[[node]]
id = "impl"
title = "cài đặt"
agent = "codex"
task = "Thêm middleware auth theo mô tả của node scan."
deps = ["scan"]
verify = "cargo test -q"      # đỏ là node HỎNG, bất kể agent nói gì
```

## Bốn quyết định đáng chú ý

**1. Verifier có quyền phủ quyết agent.** Node chỉ `done` khi lệnh `verify`
xanh. Lần chạy thật đầu tiên cho thấy vì sao: agent bị chặn quyền, không tạo
được file, nhưng vẫn trả `is_error:false` — hệ báo "OK" trong khi chẳng có gì
xảy ra. Không có verifier thì ta chỉ đang tin lời model.

**2. Protocol đi qua system prompt, không qua thân task.** Ban đầu chỉ dẫn
"ghi mutation vào `.agentgraph/mutations.jsonl`" được nối vào task. Claude
thật **nhận diện đó là prompt injection và từ chối** — phản ứng đúng. Chỉ dẫn
của operator phải đi qua kênh operator (`--append-system-prompt`).

**3. Agent đề nghị, orchestrator quyết định.** Mutation được validate trước khi
áp: id phải an toàn đường dẫn, agent phải có adapter, graph phải không có chu
trình, và có trần số node. Mọi đề nghị **bị từ chối vẫn vào log kèm lý do** —
đó là dữ liệu cho biết prompt đang dạy agent sai ở đâu.

**4. Worktree và branch mang tên run.** `ag/<run-id>/<node>`. Chạy lại trên
cùng repo không đụng branch sót lại của lần trước — bug này lộ ra ngay lần
chạy thứ hai. `.agentgraph/` không bao giờ được commit, nếu không file
mutation sẽ theo branch merge sang node join và bị áp lại.

**5. Một phép fold, ba mặt.** TUI, web và `replay` đều gọi đúng
`core::view::View::apply`. Web nhận `Patch` **đã fold sẵn** từ server chứ
không nhận event thô — nếu để client tự fold thì sẽ có hai bản cùng một quy
tắc, một Rust một JS, và chúng chắc chắn sẽ lệch. `Patch` cũng không mang
theo log để mỗi cập nhật không đẩy lại toàn bộ lịch sử.

**6. Cùng một khái niệm, hai schema khác nhau.** `file_change` của codex phát
ra dạng mảng `[{path, kind}]` trên stream `--json`, nhưng ghi dạng map
`{path: {type, content}}` trong file session. Adapter đỡ cả hai. Bài học: đọc
schema từ lần chạy thật, và đừng cho rằng hai kênh của cùng một công cụ nói
cùng một thứ tiếng.

## Ranh giới còn bỏ ngỏ

- Chi phí codex là **ước lượng** từ token; codex không trả USD như claude.
- `bypassPermissions` là cần thiết để chạy không giám sát, và nó bỏ mọi kiểm
  tra quyền. Chỉ dùng với worktree dùng một lần.
- Merge đụng độ giờ được báo cho agent qua `protocol` (kênh operator) thay vì
  chỉ nằm im trong Note của event log — nhưng agent vẫn phải TỰ xử lý đụng độ
  (đọc branch, merge tay); orchestrator không tự động giải quyết.
- Huỷ giữa chừng (q trong TUI, Ctrl-C) giết cả process group của tiến trình
  agent VÀ của verifier (không chỉ agent) — đã test bằng tiến trình `sleep`
  thật. Vẫn có khoảng trễ vài trăm ms giữa lúc gửi tín hiệu và lúc `kill
  -KILL` có hiệu lực; không có gì đảm bảo tuyệt đối cho tiến trình chết ngay
  lập tức trước khi kịp đẻ thêm con.
- Ctrl-C thật (SIGINT, khác phím `q` trong TUI) ở `--plain`/`--web` chờ
  `run_finished` ghi xong rồi mới thoát, có trần 30 giây phòng khi event log
  kẹt — không còn ngủ cố định 3 giây bất kể đã xong hay chưa.
- `agentgraph ask` resume agent cũ đúng session/worktree đã lưu, nhưng chưa
  kiểm được bằng agent thật là claude/codex thật sự hiểu ngữ cảnh cũ tốt tới
  đâu — chỉ xác nhận cờ CLI đúng và luồng orchestrator chạy được.
- `agentgraph runs` chỉ đọc log trên đĩa hiện có; không dọn hay nén log cũ.
