# Kế hoạch: agentgraph — multi-agent, graph động trong harness động

**Bắt đầu:** 2026-09-14 · **Trạng thái:** mốc 1–3 xong, ba năng lực lõi đã kiểm chứng bằng agent thật

## Bối cảnh

Khác `../harness-graph` (Python, tự gọi Claude API, tự chạy loop). Ở đây mỗi
node là **một tiến trình agent có sẵn** (claude code, codex) vốn đã có harness
riêng. Hệ này là **meta-harness**: điều phối, cô lập, kiểm chứng, và cho phép
graph tự mọc lúc chạy.

## Quyết định đã chốt

| Quyết định | Chốt | Vì sao |
| --- | --- | --- |
| Ngôn ngữ | Rust 1.95, workspace 2 crate | core tách khỏi UI để thêm web sau |
| Giao diện | TUI ratatui trước | một binary, chạy cạnh claude/codex |
| Điều khiển agent | headless NDJSON + resume | parse được tiến độ/cost; giữ session cho sub-agent bền |
| Cô lập | git worktree mỗi node | rẻ, song song an toàn, diff từng agent tách bạch |
| Tự sửa | spawn node + write skill/memory | có giá trị thật mà vẫn chặn được |
| Trạng thái | event log JSONL append-only | TUI chỉ là người đọc log; replay dựng lại y nguyên |
| Kênh protocol | `--append-system-prompt` | nhét vào task bị agent coi là prompt injection (đã gặp thật) |

## Mốc

- [x] **Mốc 1 — lõi chạy được.** ids/event/graph/harness/workspace/run,
      adapter claude + codex + fake, TUI, doctor, verifier. 32 test xanh.
      Đã chạy thật với claude: tạo file, verifier xanh, commit vào branch riêng.
- [x] **Mốc 2 — graph nhiều node thật.** claude + codex chạy SONG SONG, mỗi
      cái một worktree, verifier riêng đều xanh; node join merge được cả hai
      nhánh — chứng minh bằng verifier `grep ALPHA a.txt && grep BETA b.txt`
      xanh trong worktree của review. Tổng $0.19. TUI đã render và kiểm chứng
      bằng pty capture (điều hướng + con trỏ chọn).
- [x] **Mốc 3 — ba năng lực lõi, agent THẬT.** claude tự spawn một node codex
      (graph mọc 1→2 lúc chạy), node con thấy được việc node cha, và
      `write_skill` ghi vào `.agentgraph/skills/` rồi được nạp lại ở lần sau.
      Tổng $0.39. Hai bug nặng lộ ra ở đây, xem mục dưới.

- [x] **Mốc 3b — resume.** Lệnh `agentgraph ask <events> <node> "<câu hỏi>"`
      đọc log tìm agent/session/worktree, resume qua đúng `AgentAdapter`
      (`--resume` cho claude, `codex exec resume` cho codex — xác nhận bằng
      `--help` thật, không đoán). `EventKind::NodeFinished` giờ mang `session`
      (`#[serde(default)]`, log cũ vẫn replay được).
- [x] **Mốc 4 — web UI.** Core đã UI-agnostic; axum + SSE đọc cùng event log.
- [ ] **Mốc 5 — grind/budget theo node.** Hiện chỉ có trần toàn cục + timeout.
- [x] **Mốc 6 — merge đụng độ báo cho agent.** Trước đây chỉ `git merge
      --abort` rồi ghi Note — agent không đọc event log nên chạy tiếp trên
      cây thiếu việc của dep mà không biết. Giờ tên branch đụng độ đi vào
      `protocol` (kênh operator), cùng chỗ với hướng dẫn mutation.
- [x] **Mốc 7 — huỷ giữa chừng không mồ côi tiến trình.** `q`/Esc/Ctrl-C
      (TUI) và Ctrl-C (`--plain`/`--web`) đẩy tín hiệu qua
      `tokio::sync::watch` tới mọi adapter đang chạy; adapter giết cả process
      group (`process_group(0)`, giống cách `run.rs` đã làm cho verifier).
      Test bằng tiến trình `sleep` thật, kiểm PID biến mất sau khi huỷ.
- [x] **Mốc 8 — `agentgraph runs`.** Liệt kê `.agentgraph/runs/*/events.jsonl`,
      mới nhất trước, dùng lại `core::view::View` để fold — không viết logic
      đếm thứ hai. Log hỏng/dở dang không làm lệnh chết.

## Hai bug nặng lộ ra khi thử với agent thật

1. **Chuỗi một dep không merge.** `run.rs` chỉ merge nhánh cha khi node có
   HƠN một dep, nên hình dạng phổ biến nhất — `làm → review` — cho người
   review nhìn vào cây trống. Mọi test cũ dùng `isolate = "shared"` nên che
   mất. Đã sửa, có test chạy trong git repo thật.

2. **Agent phớt lờ `write_skill`.** Được bảo "ghi lại quy trình", claude ghi
   vào `.claude/skills/` của chính nó — prior của agent thắng protocol, và
   công sức mất sạch khi node kết thúc. Đã sửa bằng cách gọi đích danh những
   chỗ KHÔNG được ghi, kèm lệnh copy-paste sẵn. Xác nhận lại bằng agent thật.

## Đợt sửa 2026-09-15

- `Mutation::Spawn` có `verify`; chuỗi rỗng bị từ chối vì `sh -c ""` luôn xanh.
  Chạy thật: agent tự kèm `python3 check_util.py`, verifier chạy trên node con.
- Cảnh báo "không có verify" từng emit TRƯỚC `NodeAdded`, nên fold bỏ qua và
  nó vô hình trên TUI/web. `NodeAdded` của node spawn cũng ghi sai dep (bỏ
  mất `after`). Test mới kiểm qua `View`, đúng thứ giao diện thấy.
- Con trỏ đọc mutation khoá theo node, nên ở chế độ `shared` node chạy sau
  áp lại mutation của node trước. Giờ khoá theo đường dẫn file.

## Đợt 2026-09-15 (chiều)

- `ask` với codex hỏng hoàn toàn: `codex exec resume` từ chối `--sandbox` và
  `-C` (thoát mã 2). Tách `codex_args` thành hàm thuần có test; sandbox đi qua
  `-c sandbox_mode=...`, thư mục qua cwd tiến trình. Chạy thật: trả lời đúng.
- Code không compile được trên Windows (`process_group` chỉ có trên Unix).
  Gom quản lý tiến trình vào `agent/mod.rs` với bản Unix/Windows; build sạch
  cho `x86_64-pc-windows-gnu`, CHƯA chạy thật trên Windows.
- `runs` đếm thêm node chưa chạy / đang chạy.
- Repo chưa có commit: lỗi git thô → thông báo chỉ cách sửa; `doctor` thôi
  báo ✓ sai.
- Bỏ phím `f` của TUI (đổi một biến không ai đọc).
- README viết lại thành hướng dẫn từng bước + 6 công thức; mọi plan mẫu đã
  chạy được bằng fake agent.
- Phát hiện nhưng chưa làm: worktree/branch/log không bao giờ tự dọn
  (`Worktrees::remove` và `Limits::keep_branches` không ai dùng).

## Rủi ro còn mở

- `bypassPermissions` cần thiết để agent làm việc không giám sát, nhưng đó là
  bỏ mọi kiểm tra quyền. Chỉ dùng trong worktree dùng một lần.
- Chi phí codex là **ước lượng** từ token (codex không trả USD). Không được
  trộn lẫn với số thật của claude khi báo cáo.
- Merge SẠCH đã thử và chạy được thật. Merge ĐỤNG ĐỘ giờ báo cho agent qua
  `protocol` (test tự động, hai node fake ghi cùng file khác nội dung) —
  nhưng agent vẫn phải TỰ xử lý đụng độ, orchestrator không tự merge tay.
- Node do agent spawn có `verify` nhưng là TUỲ CHỌN. Không kèm thì node vẫn
  chạy và log ghi "chỉ tin lời agent". Agent thật (claude sonnet) đã tự kèm
  verify khi được protocol dạy, nhưng không có gì bắt buộc nó.
- Chế độ `shared`: các node chạy ĐỒNG THỜI dùng chung một file mutation, nên
  node spawn được gán cho node nào đọc file trước. Mỗi dòng chỉ áp một lần,
  nhưng quan hệ cha–con có thể sai. Muốn đúng thì dùng `worktree` (mặc định).
- Huỷ giữa chừng giờ giết cả process group của agent (test bằng `sleep`
  thật). Chưa test được việc bấm Ctrl-C thật trên terminal `--plain`/`--web`
  từ một test tự động (gửi SIGINT rồi quan sát toàn bộ luồng CLI là việc khó
  làm tất định trong CI) — chỉ xác nhận bằng đọc code + test ở tầng Runner.
- `agentgraph ask`: đã xác nhận cờ CLI thật (`claude --help`,
  `codex exec resume --help`) và test lỗi/log ở tầng CLI bằng log viết tay.
  Resume thật với claude (`--model sonnet`), ghi một "số bí mật" ở lượt chạy
  gốc rồi hỏi lại ở worktree cũ — trả lời đúng số, xác nhận resume thật sự
  giữ ngữ cảnh. Tổng chi phí phiên xác nhận (2 lượt run + resume) ~$0.17,
  trong trần $0.30.
- Bug thật bắt được ngay ở lần chạy `ask` thật đầu tiên: request không ai giữ
  `Sender` huỷ (kênh dùng một lần) khiến `cancel_rx.changed()` trả `Err` NGAY
  LẬP TỨC (sender rớt) — `select!` cũ coi bất kỳ lần `changed()` hoàn thành
  nào (kể cả lỗi) là "đã huỷ", nên agent bị giết trước khi kịp chạy. Sửa bằng
  `agent::wait_for_cancel` (treo vĩnh viễn khi sender rớt thay vì báo huỷ),
  có test hồi quy `khong_ai_giu_sender_thi_khong_duoc_coi_la_da_huy`.

## QA vòng hai (2026-09-15) — nghi ngờ 79 test xanh, tìm thêm 2 bug thật

- **Ctrl-C thật (SIGINT) ở `--plain`/`--web` ngủ cố định 3 giây rồi
  `std::process::exit(130)`, bất kể đã xong hay chưa.** Xác nhận bằng SIGINT
  thật (`kill -INT`) vào tiến trình `--web` với graph đã chạy xong: luôn mất
  ~3.0s để thoát dù không còn gì phải giết — `sigint_web_khong_ngu_co_dinh_khi_khong_con_gi_de_giet`
  (`crates/cli/tests/cli.rs`) đỏ (3.02s) trước khi sửa. Sửa: chờ `run_finished`
  thật qua kênh event log, trần an toàn 30s thay vì ngủ mù. Sau sửa test
  xanh trong 0.2s. (`crates/cli/src/main.rs`, hàm `run`.)
- **Bug gốc nghiêm trọng hơn: verifier hoàn toàn không nghe được tín hiệu
  huỷ.** Hai nguyên nhân cộng lại:
  1. `verify()` trong `run.rs` không hề subscribe kênh `cancel_tx` — chỉ có
     trần `node_timeout` (mặc định 30 phút). Huỷ giữa lúc verifier đang chạy
     (`cargo test` lâu chẳng hạn) bị lờ đi tới hết giờ thay vì dừng ngay.
  2. Ngay cả khi sửa (1), `canceller().send(true)` gọi đúng lúc KHÔNG còn
     receiver nào sống (không có agent nào đang `select!` trên
     `req.cancel` — ví dụ đúng lúc verifier chạy, hoặc giữa hai node) thì
     `tokio::sync::watch::Sender::send` trả lỗi và **bỏ qua giá trị gửi**,
     khác hẳn `send_replace`. `cancelled` không bao giờ thành `true`, huỷ
     câm lặng mất tác dụng dù không có lỗi nào hiện ra.
  Test hồi quy `huy_giua_chung_giet_ca_verifier_dang_chay`
  (`crates/core/tests/scheduler.rs`) đỏ trước khi sửa (verifier chạy tới hết
  `node_timeout` 20s thay vì bị huỷ ngay — hoặc `canceller.send` panic với
  `SendError` khi chưa sửa nguyên nhân (2)). Sửa: giữ một receiver sống suốt
  đời `Runner` (field `_cancel_guard`) để `send` không bao giờ câm lặng, và
  cho `verify()` `select!` trên `wait_for_cancel` giống agent, giết process
  group của verifier ngay khi bị huỷ. Sau sửa test xanh trong 0.8s, không để
  lại tiến trình mồ côi (kiểm bằng file đánh dấu không xuất hiện).
- Đã kiểm thêm, KHÔNG thấy bug: huỷ trong TUI (`q`) — `run_finished` ghi
  xong trước khi tiến trình thoát, không orphan process (kiểm bằng pty thật
  + `ps`); huỷ khi còn node chưa chạy — node đó dừng ở state `blocked`, không
  crash, nhưng KHÔNG được đếm vào `done`/`hỏng`/`bỏ qua` trong `runs` (tổng
  nhỏ hơn số node thật) — hành vi có từ trước (cũng xảy ra khi chạm ngân
  sách), không phải bug mới của tính năng huỷ, chưa sửa vì ngoài phạm vi bốn
  tính năng đang audit; merge đụng độ với một trong nhiều dep, và với node
  `isolate = shared` (đúng là không merge, vì cwd dùng chung không có gì để
  merge); `ask` với node `shared`, node do agent spawn, node hỏng, log/thư
  mục `runs` rỗng hoặc lạ tên — tất cả kiểm thủ công bằng fake adapter thật
  (không tốn tiền) đều đúng như mô tả.
- CHƯA kiểm được: `ask` resume một session **codex thật** (chỉ kiểm được
  qua code review + test log giả — không chạy agent thật để tiết kiệm ngân
  sách phiên, xem báo cáo QA cuối buổi).

## Chạy lại kiểm chứng

```bash
cargo test                      # 32 test, không cần API key
cargo run -p agentgraph-cli -- doctor
cd /tmp && mkdir t && cd t && git init -q && ...   # xem README
```
