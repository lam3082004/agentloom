# Bộ harness UI/UX

Bộ này biến agentloom từ công cụ điều phối agent lập trình thành công cụ làm
việc cho người thiết kế sản phẩm: nghiên cứu, phản biện thiết kế, phân tích dữ
liệu người dùng, bản đồ hành trình, kiến trúc thông tin, đề xuất giải pháp và
kế hoạch dự án.

Nó gồm bốn thứ:

| Thành phần | Là gì | Ở đâu sau khi cài |
| --- | --- | --- |
| **4 skill** | Nguyên tắc luôn được chèn vào **mọi** agent: không bịa, tách quan sát–insight–đề xuất, thang đánh giá giao diện, cách xử lý bằng chứng | `.agentloom/skills/` |
| **7 plan** | Mỗi plan là một graph nhiều agent cho một đầu việc | `uiux/plans/` |
| **1 verifier** | Chặn bài nộp rỗng — thứ thay cho "test xanh" khi sản phẩm là tài liệu | `uiux/kiem_tai_lieu.py` |
| **2 thư mục** | `dau-vao/` bạn bỏ dữ liệu thô vào, `ket-qua/` agent ghi kết quả ra | project của bạn |

## Vì sao phải có verifier

Agent viết tài liệu rất trôi chảy, và trôi chảy không có nghĩa là có thật. Nếu
không có bên kiểm chứng, agent nói "đã phân tích xong" là hệ thống tin ngay.

`kiem_tai_lieu.py` không chấm hay–dở (máy không làm được việc đó). Nó chặn
những kiểu rỗng **đếm được**: thiếu mục bắt buộc, chưa đủ độ dài, bảng không có
hàng dữ liệu nào, còn `TODO`/`lorem ipsum`, và — quan trọng nhất — **có số liệu
mà cả tài liệu không có lấy một nguồn**. Node nào trượt thì bị đánh hỏng, và
node phụ thuộc nó bị bỏ qua kèm lý do, thay vì để bạn phát hiện lúc đang họp.

## Cài

```bash
git clone -b uiux-kit https://github.com/lam3082004/agentloom.git
cd agentloom
./kits/uiux/cai-dat.sh ~/du-an/san-pham-cua-ban --commit
```

Project của bạn phải là **gốc** một git repo đã có ít nhất một commit — script
kiểm và báo rõ nếu chưa. Lý do: agentloom cho mỗi agent một `git worktree`
riêng tách từ HEAD, nên **file chưa commit thì agent không nhìn thấy**. Đây là
chỗ dễ vấp nhất; `--commit` để script commit hộ.

## Dùng

```bash
# 1. Bỏ dữ liệu thô vào dau-vao/ rồi commit
cp ~/phong-van/*.md ~/du-an/san-pham-cua-ban/dau-vao/
cd ~/du-an/san-pham-cua-ban && git add dau-vao && git commit -m "dữ liệu phỏng vấn đợt 1"

# 2. Mở plan, thay <CHỦ ĐỀ>, <BÀI TOÁN>... bằng bài toán thật của bạn
$EDITOR uiux/plans/3-phan-tich-nghien-cuu.toml

# 3. Chạy và xem các agent làm việc trực tiếp
agentloom run uiux/plans/3-phan-tich-nghien-cuu.toml --root . --web
```

Kết quả nằm trên branch của node cuối. Lấy về:

```bash
agentloom runs                     # xem lượt vừa chạy
git checkout al/<run-id>/bao-cao -- ket-qua/   # lấy tài liệu về nhánh làm việc
```

Muốn hỏi thêm agent vừa viết báo cáo ("vì sao bạn xếp vấn đề này mức nghiêm
trọng?"), bấm agent đó trên dashboard rồi **💬 Hỏi lại agent này** — nó resume
đúng phiên cũ, còn nguyên ngữ cảnh.

## Bảy plan

| Plan | Graph | Dùng khi |
| --- | --- | --- |
| `1-nghien-cuu-thu-cap` | 3 hướng song song (thị trường · chuẩn/guideline · người dùng) → tổng hợp | Desk research đầu dự án |
| `2-danh-gia-thiet-ke` | 3 lăng kính song song (khả dụng · tiếp cận · web-mobile) → backlog ưu tiên | Design critique một màn hình/luồng |
| `3-phan-tich-nghien-cuu` | định tính ‖ định lượng → báo cáo đối chiếu | Có transcript phỏng vấn, kết quả khảo sát |
| `4-ban-do-hanh-trinh` | giai đoạn ‖ điểm chạm → điểm đứt gãy đa kênh | Customer Journey Map, omnichannel |
| `5-kien-truc-thong-tin` | sitemap → nhãn và kế hoạch kiểm chứng | Sitemap/IA, đặt lại nhãn |
| `6-de-xuat-giai-phap` | 3 hướng độc lập (an toàn · thiết kế lại · phá cách) → so sánh, khuyến nghị | Bí ý tưởng, hoặc cần phương án để bàn |
| `7-ke-hoach-du-an` | kế hoạch → phản biện kế hoạch | Lập timeline, roadmap |

Hai chỗ đáng chú ý trong cách chia graph:

- **Plan 6 cho ba agent nghĩ độc lập rồi mới so sánh.** Một agent nghĩ ba
  phương án sẽ neo cả ba vào phương án đầu tiên. Ba agent song song thì không,
  và mỗi hướng nằm trên một branch riêng nên bạn lấy nguyên hướng mình chọn
  hoặc ghép từng phần.
- **Plan 7 có một agent chuyên phản biện kế hoạch** của agent trước. Kế hoạch
  do AI viết luôn trơn tru đáng ngờ: không ai nghỉ phép, không phụ thuộc bên
  thứ ba, mọi ước lượng vừa khít.

## Chỉnh cho vừa việc của bạn

- **Ngưỡng verify** nằm ngay trong dòng `verify` của mỗi node. Thấy nó khắt khe
  quá (hoặc dễ quá) thì sửa `--tu-toi-thieu`, `--bang-toi-thieu`,
  `--nguon-toi-thieu`. Chạy `python3 uiux/kiem_tai_lieu.py --tu-kiem` để chắc
  verifier vẫn hoạt động sau khi bạn sửa.
- **Model**: node tổng hợp và node phản biện đang để `opus` (việc suy luận
  dài), node thu thập để `sonnet` cho rẻ. Đổi tuỳ ngân sách.
- **Skill là nơi đặt quy ước chung của bạn** — ví dụ design system của công ty,
  cách gọi tên màn hình, giọng văn báo cáo. Thêm file `.md` vào
  `.agentloom/skills/` là mọi agent từ lần chạy sau đều biết. Tối đa 8 file
  được chèn, nên giữ chúng ngắn.
- Agent cũng tự ghi skill được trong lúc chạy (`write_skill`), nên quy trình
  nào lặp lại nhiều lần sẽ tự tích luỹ.

## Giới hạn thật, nên biết trước

- **Agent không nhìn được ảnh.** Bỏ file Figma hay ảnh chụp màn hình vào
  `dau-vao/` thì agent không "thấy" — phải mô tả màn hình bằng chữ, hoặc dán
  nội dung/luồng dưới dạng text. Đây là giới hạn của chính agentloom hiện tại,
  không phải của bộ này.
- **Desk research cần agent có công cụ web.** Nếu `claude` chạy không có quyền
  truy cập internet, plan 1 vẫn chạy nhưng agent chỉ dùng kiến thức nền — nó
  được dặn phải ghi rõ điều đó và hạ độ tin cậy xuống Thấp. Hãy đọc mục
  "Phương pháp" trước khi tin kết quả.
- **Verifier đếm được chứ không hiểu được.** Nó chặn bài rỗng, không chặn được
  bài sai mà đủ mục. Bạn vẫn là người đọc và quyết định — bộ này rút ngắn
  chặng từ dữ liệu thô tới bản nháp tốt, không thay bạn ký duyệt.
- Chi phí thật: mỗi plan chạy 2-4 agent, phần lớn là `sonnet`. Đặt
  `--budget` nhỏ cho lần chạy đầu để ước lượng.
