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

## Chạy lại kiểm chứng

```bash
cargo test                      # 32 test, không cần API key
cargo run -p agentgraph-cli -- doctor
cd /tmp && mkdir t && cd t && git init -q && ...   # xem README
```
