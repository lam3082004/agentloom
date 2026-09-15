# agentloom

Điều phối nhiều coding agent (**claude code**, **codex**) làm việc song song
theo một **graph** — và cho phép chính các agent tự mọc thêm việc trong lúc
chạy. Viết bằng Rust. Xem tiến độ bằng TUI trong terminal hoặc bằng trình duyệt.

agentloom **không tự gọi model**. Mỗi node là một tiến trình agent có sẵn chạy
headless trên máy bạn. Việc của agentloom là: **lập lịch** node nào chạy trước
sau, **cô lập** mỗi agent trong một git worktree riêng, **kiểm chứng** kết quả
bằng lệnh thật, và **ghi lại** mọi thứ để xem lại hoặc hỏi lại.

## Cách nhanh nhất: giao diện web

```bash
agentloom web
```

```
web đang chạy: http://127.0.0.1:7878/?token=caf92286...
```

Mở **đúng link đó** (có `?token=`) trong trình duyệt, rồi:

Bấm **+ Chạy mới** ở góc trên, rồi trong hộp thoại:

1. **Thư mục project** — nhập đường dẫn project cần làm việc. Ô bên dưới báo
   ngay nó có phải git repo đã có commit chưa.
2. **Agent chính** — `claude` hoặc `codex`.
3. **Model** — `sonnet` / `opus` / `fable` cho claude, hoặc tự nhập tên model.
4. **Prompt** — viết việc cần làm cho **một** agent chính.
5. Bấm **Chạy**.

Agent chính có thể tự giao việc cho agent con. Dashboard chia làm bốn vùng:

```
┌ agentloom  ▾lượt chạy  ● ĐANG CHẠY  mục tiêu…   0:42 · 3 đang chạy · 2 xong · 1 hỏng · $0.31  [■ Dừng] ┐
├─ AGENTS ───────────┬─ GRAPH ───────────────────────────────────────┬─ CHI TIẾT AGENT ─┤
│ ai đang chạy đứng  │  thẻ agent ──▶ thẻ agent ──▶ thẻ agent          │ model · thời gian │
│ đầu, kèm việc đang │           ╰──▶ thẻ agent                        │ chi phí · token   │
│ làm và đồng hồ     ├─ HOẠT ĐỘNG TRỰC TIẾP ───────────────────────────┤ thư mục worktree  │
│                    │ 12:04:31  sua-api  → Edit src/api.rs            │ log đầy đủ        │
└────────────────────┴─────────────────────────────────────────────────┴───────────────────┘
```

- **Thanh trên**: trạng thái lượt chạy, đồng hồ tổng, số agent theo trạng thái, chi phí, nút **Dừng**.
- **Agents**: mọi agent, người đang chạy xếp lên đầu, kèm việc đang làm.
- **Graph**: mỗi agent một thẻ (badge `C` claude, `X` codex), mũi tên nối agent với
  agent nó phụ thuộc, `⚙` đánh dấu agent do agent khác tự tạo. Kéo để di chuyển,
  cuộn để zoom, nút `⤢` để vừa khung.
- **Hoạt động trực tiếp**: mọi việc của mọi agent theo thời gian, mới nhất lên đầu;
  tích "chỉ agent đang chọn" để lọc.
- **Chi tiết agent**: bấm một thẻ hoặc một dòng để xem model, thời gian, chi phí,
  token, thư mục worktree (có nút chép) và log đầy đủ.

> Đừng mở thẳng file `crates/cli/assets/index.html` — trang cần server phía
> sau và sẽ báo "không nối được tới agentloom". Luôn mở link server in ra.

Muốn lặp lại cùng một quy trình, hay chạy trong CI, thì viết plan TOML — xem
[mục 3](#3-lượt-chạy-đầu-tiên-từng-bước) trở đi.

---

## Mục lục

1. [Cài đặt](#1-cài-đặt)
2. [Năm khái niệm cần biết](#2-năm-khái-niệm-cần-biết)
3. [Lượt chạy đầu tiên, từng bước](#3-lượt-chạy-đầu-tiên-từng-bước)
4. [Viết plan](#4-viết-plan)
5. [Xem tiến độ và kết quả](#5-xem-tiến-độ-và-kết-quả)
6. [Đưa công việc của agent vào code của bạn](#6-đưa-công-việc-của-agent-vào-code-của-bạn)
7. [Hỏi lại một agent đã chạy xong](#7-hỏi-lại-một-agent-đã-chạy-xong)
8. [Áp dụng vào công việc: 6 công thức](#8-áp-dụng-vào-công-việc-6-công-thức)
9. [Viết `verify` cho tốt](#9-viết-verify-cho-tốt)
10. [Chi phí, quyền và an toàn](#10-chi-phí-quyền-và-an-toàn)
11. [Dọn dẹp](#11-dọn-dẹp)
12. [Xử lý sự cố](#12-xử-lý-sự-cố)
13. [Tham chiếu lệnh](#13-tham-chiếu-lệnh)
14. [Kiến trúc và quyết định thiết kế](#14-kiến-trúc-và-quyết-định-thiết-kế)
15. [Giới hạn đã biết](#15-giới-hạn-đã-biết)

---

## 1. Cài đặt

**Cần có:** Rust ≥ 1.85, `git`, và ít nhất một trong hai agent `claude` hoặc
`codex` đã đăng nhập sẵn.

```bash
git clone https://github.com/lam3082004/agentloom.git
cd agentloom
cargo install --path crates/cli --locked
```

Lệnh trên đặt binary `agentloom` vào `~/.cargo/bin`.

> **Lỗi hay gặp:** `cargo build --release` **không** cài gì vào `PATH` — nó chỉ
> tạo file `target/release/agentloom`. Gõ `agentloom` sau lệnh đó sẽ báo
> `command not found`. Dùng `cargo install` như trên, hoặc gọi thẳng
> `./target/release/agentloom`.

Kiểm tra `~/.cargo/bin` đã nằm trong `PATH` chưa, rồi chạy `doctor`:

```bash
echo "$PATH" | tr ':' '\n' | grep cargo     # phải thấy .../.cargo/bin
agentloom doctor
```

```
  ✓ claude   claude → /home/you/.local/bin/claude
  ✓ codex    codex → /home/you/.nvm/.../bin/codex
  ✓ fake     true → /usr/bin/true

  ✓ git
  ✓ thư mục hiện tại là git repo — cô lập worktree bật
```

Dòng cuối phụ thuộc thư mục bạn đứng khi chạy `doctor` — chạy nó **trong
project mà bạn định cho agent làm việc**.

---

## 2. Năm khái niệm cần biết

| Khái niệm | Nghĩa là |
| --- | --- |
| **Plan** | File TOML liệt kê các việc (node) và việc nào phải đợi việc nào. Đó là điểm xuất phát — graph thật có thể mọc thêm. |
| **Node** | Một việc, giao cho **một** agent (`claude` hoặc `codex`). |
| **Dep** | `deps = ["a"]` nghĩa là node này chỉ chạy khi `a` đã **xong**. Nhiều node không phụ thuộc nhau thì chạy **song song**. |
| **Worktree** | Mỗi node làm việc trên một bản sao riêng của repo, trên branch `al/<run-id>/<node>`. Agent song song không bao giờ đè file của nhau. Node có dep được **merge sẵn** công việc của các dep trước khi chạy. |
| **Verify** | Lệnh shell chạy sau khi agent xong. Thoát `0` thì node **xong**; khác `0` thì node **hỏng** — bất kể agent nói gì. |

Và một điều khiến agentloom là "động": **agent đang chạy có thể tự spawn node
mới** (giao việc cho agent khác), **ghi quy trình** để lần sau dùng lại, và
**ghi memory**. Mọi đề nghị đều được validate trước khi áp dụng.

---

## 3. Lượt chạy đầu tiên, từng bước

Ví dụ: một project Python có hàm bị sai, muốn agent sửa và **chứng minh** đã sửa
đúng.

### Bước 1 — Đứng trong project của bạn, không phải trong repo agentloom

```bash
cd ~/code/project-cua-ban
```

> Chạy `agentloom run` bên trong chính repo agentloom sẽ tạo worktree và
> branch `al/*` ngay trong repo đó. Luôn chạy trong project cần làm việc.

### Bước 2 — Project phải là git repo có ít nhất một commit

```bash
git status                         # phải là git repo
git log --oneline -1               # phải có ít nhất một commit
```

Nếu chưa có:

```bash
git init
git add -A && git commit -m init
```

Worktree tách nhánh từ commit hiện tại, nên repo vừa `git init` mà chưa commit
sẽ làm mọi node hỏng. Commit hết thay đổi đang dở trước khi chạy — agent chỉ
thấy những gì **đã commit**.

### Bước 3 — Viết plan

Tạo file `plan.toml` ở gốc project:

```toml
goal = "sửa hàm add trong calc.py"

[[node]]
id = "sua-add"
title = "sửa hàm add"
agent = "claude"
model = "sonnet"
task = "Hàm add trong calc.py cộng sai. Sửa nó cho đúng. Chỉ sửa calc.py."
verify = "python3 -c 'from calc import add; assert add(2, 3) == 5'"
```

### Bước 4 — Chạy

```bash
agentloom run plan.toml --permission-mode bypassPermissions
```

Vì sao cần `bypassPermissions`: ở chế độ mặc định `acceptEdits`, claude chạy
headless không có ai bấm "cho phép", nên các lệnh shell và một số thao tác ghi
file sẽ bị chặn — agent chạy xong mà không làm được gì. Chế độ này an toàn tương
đối vì agent làm trong **worktree riêng**, không đụng vào thư mục làm việc của
bạn. Xem [mục 10](#10-chi-phí-quyền-và-an-toàn) trước khi dùng trên repo quan
trọng.

### Bước 5 — Xem trên TUI

```
┌ graph · 1 node ──────────────┐┌ sua-add — sửa hàm add · claude · running ─┐
│▸ ◐ sua-add  claude           ││→ Read calc.py                             │
│                              ││→ Edit calc.py                             │
│                              ││· Đã sửa: a - b → a + b                    │
└──────────────────────────────┘└───────────────────────────────────────────┘
 $0.07 · 1 chạy  0 xong  0 hỏng  0 chờ · [↑↓] node  [q] thoát
```

Khi thanh dưới hiện `XONG`, bấm `q` để thoát. Terminal in tổng kết:

```
OK · 1 xong · 0 hỏng · 0 bỏ qua · $0.07
event log: /…/project-cua-ban/.agentloom/runs/20260915-101500-a1b2c3/events.jsonl
```

### Bước 6 — Xem agent đã sửa gì, rồi đưa vào code

```bash
git branch --list 'al/*'
git diff HEAD...al/20260915-101500-a1b2c3/sua-add
git merge --no-ff al/20260915-101500-a1b2c3/sua-add
```

Chi tiết ở [mục 6](#6-đưa-công-việc-của-agent-vào-code-của-bạn).

---

## 4. Viết plan

```toml
goal = "mô tả ngắn cho cả lượt chạy"     # hiện trên TUI/web và trong `runs`

[[node]]
id = "ten-node"          # BẮT BUỘC. Chỉ a-z 0-9 - _, tối đa 64 ký tự
title = "mô tả ngắn"     # BẮT BUỘC
agent = "claude"         # BẮT BUỘC. "claude" | "codex"
task = """
Việc cần làm. Viết như giao việc cho một đồng nghiệp giỏi nhưng
chưa biết gì về bối cảnh: phạm vi, file liên quan, điều KHÔNG được làm.
"""
deps = ["node-khac"]     # tuỳ chọn. Chỉ chạy khi các node này đã XONG
model = "sonnet"         # tuỳ chọn. claude: "sonnet" | "opus" | ... ; codex: tên model
verify = "cargo test -q" # tuỳ chọn nhưng NÊN CÓ. Thoát 0 = xong
isolate = "worktree"     # tuỳ chọn. "worktree" (mặc định) | "shared"
```

**Quy tắc thứ tự:** không cần viết node theo thứ tự — node con khai báo trước
node cha vẫn chạy đúng. agentloom từ chối cả plan nếu có id trùng, dep trỏ tới
node không tồn tại, hoặc dep tạo vòng tròn, và báo rõ lý do.

**Khi node hỏng:** mọi node phụ thuộc nó (trực tiếp hay gián tiếp) bị **bỏ qua**
chứ không chạy mù trên kết quả hỏng.

**`isolate = "shared"`:** node làm thẳng trong thư mục gốc, không có worktree.
Chỉ dùng cho việc chỉ-đọc hoặc khi chắc chắn không có node nào chạy cùng lúc
ghi cùng file.

### Chạy nhanh một việc, không cần file plan

```bash
agentloom run --goal "Sửa test đang đỏ trong tests/test_api.py" --agent claude \
  --permission-mode bypassPermissions
```

Cách này tạo một node tên `root`, **không có verify** — chỉ nên dùng thử.

---

## 5. Xem tiến độ và kết quả

### TUI (mặc định)

| Phím | Tác dụng |
| --- | --- |
| `↑` `↓` hoặc `k` `j` | chọn node |
| `PageUp` `PageDown` | cuộn log của node đang chọn |
| `q`, `Esc`, `Ctrl-C` | thoát — **huỷ** mọi agent đang chạy |

Ký hiệu trạng thái: `◐` đang chạy · `●` xong · `✕` hỏng · `⊘` bỏ qua ·
`○` đang chờ. Dấu `⚙` cạnh tên = node do **agent** tạo ra lúc chạy.

Trong log: `→` là agent gọi tool, `·` là agent nói, `⚙` là đề nghị sửa graph,
`i` là ghi chú của hệ thống (kể cả kết quả verifier).

### Web

Có hai cách mở giao diện web:

| Lệnh | Dùng khi |
| --- | --- |
| `agentloom web` | muốn **chạy từ trình duyệt**: gõ prompt, chọn agent/model/thư mục, bấm Chạy. Một server giữ được nhiều lượt chạy, chọn qua ô trên cùng |
| `agentloom run plan.toml --web` | đã có plan và chỉ muốn **xem** lượt chạy đó trong trình duyệt thay vì TUI |

```bash
agentloom web --port 7878 --root ~/code/project-cua-ban
```

Server in ra link có `?token=` — mở đúng link đó. Graph xong, trang **vẫn mở**
để xem lại; `Ctrl-C` trong terminal để tắt server. Mỗi lần khởi động là một
token mới, nên link cũ hết tác dụng. Server chỉ lắng nghe `127.0.0.1`.

Trên graph: viền xanh dương có vạch chạy là agent đang làm việc (kèm đồng hồ và
việc nó đang làm), xanh lá là xong, đỏ là hỏng, mờ là bị bỏ qua vì agent trước
hỏng; mũi tên nét đứt chuyển động trỏ vào agent đang chạy. Dashboard theo giao
diện sáng/tối của hệ điều hành.

### Chạy trong CI hoặc pipe

```bash
agentloom run plan.toml --plain | tee run.jsonl
```

Mỗi dòng là một event JSON. Exit code `0` khi mọi node xong, `1` khi có lỗi.

### Xem lại lượt cũ

```bash
agentloom runs                                          # liệt kê, mới nhất trước
agentloom replay .agentloom/runs/<run-id>/events.jsonl # mở lại trong TUI
agentloom replay .agentloom/runs/<run-id>/events.jsonl --web
agentloom replay .agentloom/runs/<run-id>/events.jsonl --plain
```

```
20260915-101500-a1b2c3   OK       3 xong · 0 hỏng · 0 bỏ qua · 0 chưa chạy · $0.19 · thêm auth
20260915-093000-d4e5f6   CÓ LỖI   1 xong · 1 hỏng · 1 bỏ qua · 2 chưa chạy · $0.08 · sửa test
```

`chưa chạy` là node chưa từng được chạy vì lượt chạy bị huỷ hoặc chạm trần
ngân sách. `DỞ DANG` là lượt chạy không có dòng kết thúc (tiến trình bị giết).

---

## 6. Đưa công việc của agent vào code của bạn

agentloom **không bao giờ tự merge vào branch của bạn**. Mỗi node xong sạch có
thay đổi thì được commit lên branch riêng `al/<run-id>/<node>`. Bạn quyết định
lấy gì.

```bash
# 1. Xem có những branch nào
git branch --list 'al/*'

# 2. Xem một node đã làm gì
git log --oneline HEAD..al/<run-id>/<node>
git diff HEAD...al/<run-id>/<node>

# 3. Lấy vào branch hiện tại
git merge --no-ff al/<run-id>/<node>
```

**Mẹo với graph có node gộp:** node có dep đã chứa sẵn công việc của mọi dep.
Với graph `api` + `ui` → `review`, chỉ cần merge branch của `review` là lấy được
cả ba — nhưng chỉ khi `review` **xong** (verify xanh).

**Chỉ lấy một phần:**

```bash
git checkout al/<run-id>/<node> -- src/duong/dan/file.rs
```

---

## 7. Hỏi lại một agent đã chạy xong

Mỗi node lưu session của agent. `ask` **tiếp tục đúng cuộc trò chuyện cũ**,
trong đúng worktree cũ — agent còn nhớ nó đã đọc gì, sửa gì, vì sao.

```bash
agentloom ask .agentloom/runs/<run-id>/events.jsonl sua-add \
  "Vì sao bạn chọn sửa ở calc.py chứ không phải ở chỗ gọi hàm?"
```

Dùng khi:
- muốn hiểu **lý do** một thay đổi mà không phải đọc lại toàn bộ diff;
- review phát hiện vấn đề — nhờ chính agent đó sửa tiếp với đầy đủ ngữ cảnh;
- node hỏng — hỏi agent nó đã vướng ở đâu.

Chạy được với cả `claude` và `codex`. Worktree của node phải còn trên đĩa (xem
[mục 11](#11-dọn-dẹp) — đừng dọn trước khi hỏi xong).

---

## 8. Áp dụng vào công việc: 6 công thức

Mỗi công thức là một plan chạy được — sửa `task` và `verify` cho hợp project.

### Công thức 1 — Sửa bug có test chứng minh

**Khi nào:** có một test đỏ hoặc một bug tái hiện được bằng lệnh.

```toml
goal = "sửa lỗi đăng nhập khi email có chữ hoa"

[[node]]
id = "sua-bug"
title = "sửa lỗi email chữ hoa"
agent = "claude"
model = "sonnet"
task = """
Test tests/test_auth.py::test_login_email_hoa đang đỏ.
Tìm nguyên nhân gốc và sửa. Không sửa hay xoá test.
Không đổi hành vi của các test khác.
"""
verify = "pytest -q tests/test_auth.py"
```

Điểm mấu chốt: `verify` chạy **cả file test**, không chỉ test đang đỏ — để bắt
trường hợp agent sửa được test này nhưng làm vỡ test khác.

### Công thức 2 — Tính năng lớn chia nhỏ, làm song song, rồi review

**Khi nào:** tính năng chạm nhiều phần độc lập (backend, frontend, tài liệu).

```toml
goal = "thêm tính năng xuất báo cáo CSV"

[[node]]
id = "khao-sat"
title = "khảo sát codebase"
agent = "claude"
model = "sonnet"
task = """
Đọc code liên quan tới báo cáo trong src/reports/. Ghi ra file
docs/csv-plan.md: các hàm cần thêm, file cần sửa, rủi ro. KHÔNG sửa code.
"""
verify = "test -s docs/csv-plan.md"

[[node]]
id = "backend"
title = "API xuất CSV"
agent = "codex"
task = "Làm theo docs/csv-plan.md, phần backend: thêm endpoint GET /reports/{id}.csv."
deps = ["khao-sat"]
verify = "cargo test -q reports"

[[node]]
id = "frontend"
title = "nút tải CSV"
agent = "claude"
model = "sonnet"
task = "Làm theo docs/csv-plan.md, phần frontend: thêm nút 'Tải CSV' gọi endpoint mới."
deps = ["khao-sat"]
verify = "npm test --silent -- reports"

[[node]]
id = "review"
title = "review tổng"
agent = "claude"
model = "opus"
task = """
Backend và frontend đã được merge vào cây này. Review toàn bộ thay đổi so với
docs/csv-plan.md: thiếu gì, sai gì, lỗ hổng bảo mật. Sửa các vấn đề nhỏ;
vấn đề lớn thì ghi vào docs/csv-review.md.
"""
deps = ["backend", "frontend"]
verify = "cargo test -q && npm test --silent"
```

`backend` và `frontend` chạy **cùng lúc**. `review` chạy trên cây đã merge cả
hai — nếu hai bên đụng cùng file, agent review được báo rõ branch nào đụng độ.

### Công thức 3 — Nâng cấp / migration nhiều module song song

**Khi nào:** cùng một thay đổi lặp lại ở nhiều nơi độc lập (đổi API cũ sang
mới, nâng version thư viện, thêm type hint...).

```toml
goal = "chuyển logging sang structlog ở ba service"

[[node]]
id = "svc-billing"
title = "billing"
agent = "codex"
task = "Trong services/billing/: thay mọi logging.getLogger bằng structlog.get_logger. Chỉ sửa trong thư mục đó."
verify = "pytest -q services/billing"

[[node]]
id = "svc-orders"
title = "orders"
agent = "codex"
task = "Trong services/orders/: thay mọi logging.getLogger bằng structlog.get_logger. Chỉ sửa trong thư mục đó."
verify = "pytest -q services/orders"

[[node]]
id = "svc-users"
title = "users"
agent = "claude"
model = "sonnet"
task = "Trong services/users/: thay mọi logging.getLogger bằng structlog.get_logger. Chỉ sửa trong thư mục đó."
verify = "pytest -q services/users"
```

```bash
agentloom run plan.toml --parallel 3 --budget 3 --permission-mode bypassPermissions
```

Mỗi service một branch riêng: service nào hỏng không kéo theo service khác, và
bạn merge từng cái sau khi review.

### Công thức 4 — Hai agent khác nhau kiểm tra chéo

**Khi nào:** thay đổi nhạy cảm, muốn một agent **khác loại** soi lại.

```toml
goal = "thêm rate limit cho API đăng nhập"

[[node]]
id = "lam"
title = "cài rate limit"
agent = "claude"
model = "sonnet"
task = "Thêm rate limit 5 lần/phút theo IP cho POST /login. Viết test cho nó."
verify = "cargo test -q login"

[[node]]
id = "soi"
title = "codex soi lại"
agent = "codex"
task = """
Một agent khác vừa thêm rate limit cho POST /login. Tìm cách vượt qua nó:
đổi header X-Forwarded-For, đổi hoa thường đường dẫn, gửi đồng thời...
Với mỗi lỗ hổng tìm được, viết một test tái hiện nó. Không sửa code chính.
"""
deps = ["lam"]
verify = "cargo test -q login"
```

Nếu `soi` viết được test làm `verify` đỏ, node hỏng — đó chính là tín hiệu có
lỗ hổng thật. Dùng `agentloom ask` hỏi `soi` chi tiết, rồi hỏi `lam` để sửa.

### Công thức 5 — Để agent tự chia việc (graph động)

**Khi nào:** chưa biết trước có bao nhiêu việc — điều tra một sự cố, dọn một
đống cảnh báo, sửa loạt test đỏ.

```toml
goal = "dọn toàn bộ cảnh báo clippy"

[[node]]
id = "dieu-phoi"
title = "phân loại cảnh báo"
agent = "claude"
model = "sonnet"
task = """
Chạy `cargo clippy --all-targets`. Nhóm các cảnh báo theo module.
KHÔNG tự sửa. Với mỗi module có cảnh báo, spawn một node dùng agent codex để
sửa riêng module đó, kèm verify là lệnh clippy giới hạn cho module ấy.
"""
```

Agent thật sẽ tự viết các node con, ví dụ:

```json
{"op":"spawn","id":"fix-parser","agent":"codex","task":"Sửa cảnh báo clippy trong src/parser/ ...","verify":"cargo clippy -p parser -- -D warnings"}
```

Các node này hiện trên TUI với dấu `⚙` và chạy song song. Giới hạn an toàn: tối
đa 64 node mỗi lượt; id hoặc agent không hợp lệ bị từ chối và ghi lý do vào log.

### Công thức 6 — Tích luỹ quy trình cho repo

**Khi nào:** có quy trình lặp lại (release, điều tra hiệu năng, thêm endpoint
theo chuẩn team) mà muốn agent các lần sau làm giống nhau.

```toml
goal = "thêm endpoint và ghi lại quy trình"

[[node]]
id = "them-endpoint"
title = "thêm endpoint health"
agent = "claude"
model = "sonnet"
task = """
Thêm endpoint GET /health theo đúng cách các endpoint khác đang làm.
Xong việc, ghi lại quy trình bạn vừa dùng thành một skill tên `them-endpoint`
để lần sau dùng lại.
"""
verify = "cargo test -q health"
```

Quy trình được lưu ở `.agentloom/skills/them-endpoint.md` trong project. **Mọi
lượt chạy sau** trên project này đều tự nạp skill cho mọi agent — tối đa 8
skill, xếp theo tên file. Mở file ra đọc và sửa tay thoải mái — nó là markdown
thường; xoá bớt skill cũ nếu có quá 8.

---

## 9. Viết `verify` cho tốt

`verify` là thứ duy nhất chứng minh việc **thật sự** xong. Đã có lượt chạy thật
mà agent bị chặn quyền, không làm được gì, nhưng vẫn báo thành công — chỉ
`verify` bắt được.

| Tốt | Kém | Vì sao |
| --- | --- | --- |
| `pytest -q tests/test_auth.py` | `pytest -q -k test_login` | chạy rộng hơn bắt được việc làm vỡ chỗ khác |
| `cargo test -q && cargo clippy -- -D warnings` | `cargo build` | build được chưa chắc đúng |
| `grep -qx 'version = "2.0"' Cargo.toml` | `grep 2.0 Cargo.toml` | khớp chính xác, không khớp nhầm |
| `test -s docs/plan.md` | `ls docs/` | kiểm file tồn tại **và** không rỗng |

- Chạy trong worktree của node, bằng `sh -c` (Linux/macOS) hoặc `cmd /C` (Windows).
- Có cùng timeout với node (`--timeout-min`); treo quá hạn là hỏng.
- Không có `verify` thì agentloom chỉ còn tin lời agent — và ghi rõ điều đó vào log.
- **Đừng để chính agent viết `verify` cho việc của nó** mà không đọc lại — agent
  có thể viết một lệnh luôn xanh.

---

## 10. Chi phí, quyền và an toàn

### Chi phí

```bash
agentloom run plan.toml --budget 5 --parallel 2 --timeout-min 20
```

- `--budget` (mặc định `20` USD): chạm trần thì **không nạp thêm node mới**; node
  đang chạy vẫn chạy nốt.
- Chi phí **claude là số thật** do claude trả về. Chi phí **codex là ước lượng**
  từ số token — codex không báo USD.
- Thử plan mới với `model = "sonnet"` và `--budget` nhỏ trước.

Mức tham khảo từ các lượt chạy thật khi phát triển: một node claude sonnet làm
việc nhỏ khoảng `$0.07`; graph 3 node claude + codex khoảng `$0.19`.

### Quyền

| `--permission-mode` | Dùng khi |
| --- | --- |
| `acceptEdits` (mặc định) | thận trọng; agent claude chạy không giám sát thường **bị chặn** lệnh shell |
| `bypassPermissions` | chạy tự động thật sự; agent làm được mọi thứ user của bạn làm được |
| `plan` | agent chỉ lập kế hoạch, không sửa gì |

Cờ này áp cho **claude**. **codex** luôn chạy với sandbox `workspace-write`.

### An toàn

- Worktree cô lập **file**, không cô lập **quyền**: với `bypassPermissions`,
  agent vẫn chạy lệnh shell bằng quyền của bạn, vẫn đọc được `~/.ssh`, vẫn gọi
  mạng. Không chạy trên máy chứa bí mật quan trọng mà chưa hiểu điều này.
- Mọi đề nghị sửa graph của agent đều bị validate: id không thoát được thư mục,
  agent lạ bị từ chối, không tạo được vòng tròn, có trần số node.
- Huỷ (`q`, `Ctrl-C`, nút **Dừng** trên web) giết **cả cây tiến trình** của
  agent và của verifier — kể cả lệnh codex chạy trong sandbox riêng. Đã kiểm
  bằng codex thật đang chạy `sleep` trong sandbox.
- Giao diện web đòi **token** in ra terminal cho mọi API, và từ chối header
  `Host` lạ. Lý do: nút Chạy khởi động agent chạy lệnh shell, mà mọi trang web
  khác đang mở trong trình duyệt đều gửi được request tới `127.0.0.1`. Không
  chia sẻ link có token cho người khác.

---

## 11. Dọn dẹp

agentloom **không tự xoá** worktree và branch sau khi chạy — để bạn còn
review, merge và `ask`. Chúng tích luỹ dần trong `.agentloom/worktrees/`.

Dọn **một lượt chạy** (sau khi đã merge xong):

```bash
RUN=20260915-101500-a1b2c3
for w in .agentloom/worktrees/$RUN/*/; do git worktree remove --force "$w"; done
git worktree prune
git branch --list "al/$RUN/*" | tr -d ' +*' | xargs -r git branch -D
```

Dọn **tất cả**:

```bash
for w in .agentloom/worktrees/*/*/; do git worktree remove --force "$w"; done
git worktree prune
git branch --list 'al/*' | tr -d ' +*' | xargs -r git branch -D
```

Log các lượt chạy nằm ở `.agentloom/runs/` — xoá thư mục con tương ứng nếu
không cần xem lại. Skill và memory tích luỹ nằm ở `.agentloom/skills/` và
`.agentloom/memory/`; **giữ lại** nếu muốn các lượt sau dùng tiếp.

Thêm `.agentloom/` vào `.gitignore` của project để không lỡ commit chúng.

---

## 12. Xử lý sự cố

| Triệu chứng | Nguyên nhân | Cách sửa |
| --- | --- | --- |
| `command not found: agentloom` | chưa cài vào `PATH` | `cargo install --path crates/cli --locked` trong repo agentloom |
| node hỏng: `repo chưa có commit nào` | repo vừa `git init` | `git add -A && git commit -m init` |
| `doctor` báo `KHÔNG phải git repo` | đứng sai thư mục hoặc chưa `git init` | `cd` vào project; mọi node sẽ dùng chung thư mục nếu không phải repo |
| node **xong** nhưng không có gì thay đổi | agent bị chặn quyền và không có `verify` | thêm `--permission-mode bypassPermissions`, **luôn** có `verify` |
| log có `agent bị chặn quyền (… lần)` | như trên | như trên |
| agent không thấy thay đổi mới nhất của bạn | thay đổi chưa commit | commit trước khi chạy — worktree tách từ commit hiện tại |
| `merge đụng độ, agent phải tự xử lý` | hai node trước sửa cùng chỗ | bình thường; agent node sau được báo. Muốn tránh thì chia phạm vi file rõ hơn trong `task` |
| web: cổng đã bị dùng | cổng `7878` bận | `--port 7879` |
| web: "không nối được tới agentloom" | mở thẳng file `index.html` | chạy `agentloom web`, mở link nó in ra |
| web: "thiếu hoặc sai token" | mở `http://127.0.0.1:7878` không có token, hoặc server đã khởi động lại | mở lại đúng link mới nhất trong terminal |
| web: nút Chạy báo "web này chỉ để xem" | trang được mở bằng `run --web` / `replay --web` | dùng `agentloom web` |
| `ask`: worktree đã bị xoá | đã dọn dẹp trước khi hỏi | không khôi phục được session trong worktree; chạy lại node |
| chi phí codex trông thấp/cao lạ | codex chỉ báo token, USD là ước lượng | đối chiếu với trang billing của OpenAI |

Muốn xem đầy đủ chuyện gì đã xảy ra: mọi thứ nằm trong
`.agentloom/runs/<run-id>/events.jsonl`, mỗi dòng một event JSON.

---

## 13. Tham chiếu lệnh

```
agentloom doctor
agentloom run [PLAN] [--goal G] [--agent claude|codex] [--root DIR]
               [--parallel N=3] [--budget USD=20] [--timeout-min M=30]
               [--permission-mode MODE=acceptEdits] [--plain | --web [--port 7878]]
agentloom web [--port 7878] [--root DIR]
agentloom runs [--root DIR]
agentloom replay EVENTS [--plain | --web [--port 7878]]
agentloom ask EVENTS NODE QUESTION [--plain]
```

Giao thức agent dùng để sửa graph — agentloom tự dạy agent qua system prompt,
bạn không cần viết. Agent nối từng dòng JSON vào `.agentloom/mutations.jsonl`
trong worktree của nó:

```json
{"op":"spawn","id":"ten","agent":"claude|codex","task":"...","verify":"...","after":["node-khac"],"model":"sonnet"}
{"op":"write_skill","name":"ten","body":"quy trình dạng markdown"}
{"op":"write_memory","key":"ten","value":"nội dung"}
```

Node do agent spawn tự động phụ thuộc node đã spawn nó, cộng thêm các node trong
`after`.

---

## 14. Kiến trúc và quyết định thiết kế

```
crates/core/            lõi, không biết gì về giao diện
  ids.rs                  NodeId/RunId — chặn id do agent đặt phá đường dẫn
  event.rs                event log JSONL append-only + broadcast
  graph.rs                DAG động: dep, chu trình, trần số node
  view.rs                 fold event → trạng thái hiển thị, dùng chung mọi mặt
  agent/                  adapter claude · codex · fake; quản lý tiến trình đa nền tảng
  harness.rs              protocol mutation: spawn / write_skill / write_memory
  workspace.rs            git worktree mỗi node, merge khi có dep
  run.rs                  scheduler: song song, ngân sách, verifier, huỷ
  config.rs               plan TOML + trần chi phí

crates/cli/             binary `agentloom`
  tui.rs                  ratatui
  web.rs                  axum + SSE
  assets/index.html       trang web, không thư viện ngoài
  main.rs                 run · runs · replay · ask · doctor
```

**Verifier có quyền phủ quyết agent.** Lần chạy thật đầu tiên: agent bị chặn
quyền, không tạo được file, vẫn trả `is_error:false`. Không có verifier thì chỉ
đang tin lời model.

**Chỉ dẫn của hệ đi qua system prompt, không qua thân task.** Khi nối protocol
vào task, claude thật nhận diện đó là prompt injection và từ chối — phản ứng
đúng. Chỉ dẫn của operator phải đi kênh operator.

**Agent đề nghị, orchestrator quyết định.** Mutation được validate trước khi áp;
đề nghị bị từ chối vẫn vào log kèm lý do — đó là dữ liệu cho biết prompt đang
dạy agent sai ở đâu. Protocol gọi đích danh những chỗ agent hay ghi nhầm
(`.claude/skills/`, `AGENTS.md`) vì agent thật từng bỏ `write_skill` để dùng cơ
chế skill riêng của nó.

**Node có dep luôn được merge công việc của dep**, kể cả khi chỉ có một dep.
Bản đầu chỉ merge khi có từ hai dep trở lên — chuỗi "làm → review" cho người
review nhìn vào cây trống.

**Một phép fold, nhiều mặt.** TUI, web, `replay`, `runs` đều đi qua
`View::apply`. Web nhận `Patch` đã fold sẵn thay vì event thô — nếu không sẽ có
hai bản cùng một quy tắc, một Rust một JS, chắc chắn lệch nhau.

**Đọc schema từ lần chạy thật.** `file_change` của codex có dạng mảng trên
stream `--json` nhưng dạng map trong file session; `codex exec resume` từ chối
`--sandbox` và `-C` mà `codex exec` thường nhận. Cả hai chỉ lộ ra khi chạy thật.

**Huỷ đi theo cây cha–con, không theo process group.** Sandbox của codex chạy
lệnh bằng `bwrap --new-session`, tức tách sang session riêng — `kill -<pgid>`
không với tới, và lần huỷ đầu tiên với codex thật để lại `sleep` sống mồ côi.
Giờ agentloom đóng băng cả cây (`SIGSTOP`), gom lại toàn bộ hậu duệ, rồi mới
giết. Thứ tự quan trọng: giết agent trước thì con của nó bị chuyển về init và
mất dấu.

**Web có token dù chỉ lắng nghe `127.0.0.1`.** Nút Chạy khởi động agent chạy
lệnh shell. Mọi trang web khác đang mở đều gửi được request tới `127.0.0.1`, và
DNS rebinding còn đọc được phản hồi. Token in ra terminal chặn trường hợp đầu,
kiểm tra header `Host` chặn trường hợp sau.

---

## 15. Giới hạn đã biết

- **Windows mới được kiểm ở mức compile.** Code quản lý tiến trình có bản
  Windows (`CREATE_NEW_PROCESS_GROUP`, `taskkill /T /F`, `cmd /C`) và build sạch
  cho `x86_64-pc-windows-gnu`, nhưng chưa chạy thật trên máy Windows.
- **Chi phí codex là ước lượng** từ token.
- **`verify` là tuỳ chọn** với node do agent spawn; protocol dạy agent luôn kèm
  nó, nhưng không bắt buộc.
- **`isolate = "shared"`**: các node chạy đồng thời dùng chung một file mutation,
  node spawn có thể bị gán nhầm cha. Dùng `worktree` (mặc định) nếu cần chính xác.
- **Merge đụng độ không tự giải quyết** — agent node sau được báo và phải tự xử
  lý.
- **Không tự dọn** worktree, branch hay log — xem [mục 11](#11-dọn-dẹp).
- **`--budget` không cắt node đang chạy**, chỉ ngừng nạp node mới.
- **Web chỉ giữ các lượt chạy khởi động trong phiên server hiện tại.** Tắt
  server là mất danh sách; lượt cũ vẫn xem được bằng `agentloom runs` /
  `replay --web`.
