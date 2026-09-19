#!/usr/bin/env python3
"""Verifier cho tài liệu UI/UX — thứ thay cho `cargo test` khi sản phẩm là chữ.

Với việc code, "xong" nghĩa là test xanh. Với việc nghiên cứu và thiết kế,
agent rất dễ nộp một bài viết trôi chảy mà rỗng: thiếu mục bắt buộc, bịa số
liệu không nguồn, để nguyên `TODO`, hoặc viết ba dòng rồi bảo đã xong. Script
này là bên KIỂM CHỨNG trong khoảng cách sinh–kiểm: nó không chấm hay, nó chặn
những kiểu rỗng đếm được.

Thoát 0 khi tài liệu đạt, khác 0 kèm lý do khi không đạt.

    python3 uiux/kiem_tai_lieu.py ket-qua/bao-cao.md \\
        --muc "Mục tiêu" --muc "Phát hiện" --muc "Đề xuất" \\
        --tu-toi-thieu 400 --bang-toi-thieu 1 --nguon-toi-thieu 3

Tự kiểm chính nó: python3 uiux/kiem_tai_lieu.py --tu-kiem
"""
import argparse
import re
import sys
from pathlib import Path

# Chữ agent hay để lại khi chưa làm thật. Tìm cả dạng viết hoa lẫn thường.
DAU_HIEU_BO_TRONG = [
    "todo", "tbd", "lorem ipsum", "...", "chưa rõ", "điền sau", "xxx",
    "<placeholder>", "[placeholder]", "cần bổ sung sau",
]

# Câu khẳng định có số liệu mà không kèm nguồn là chỗ agent hay bịa nhất.
SO_LIEU = re.compile(r"\b\d+([.,]\d+)?\s*%|\b\d{4,}\b")
# Ngày tháng và khối metadata đầu tài liệu KHÔNG phải số liệu cần dẫn nguồn.
# Không trừ chúng ra thì mọi tài liệu đúng định dạng (header luôn có ngày
# `2026-09-19`) đều bị báo "có số liệu mà không có nguồn" — verifier kêu oan
# một lần là lần sau người ta tắt nó đi.
NGAY_THANG = re.compile(r"\b\d{4}-\d{2}-\d{2}\b|\b\d{1,2}/\d{1,2}/\d{4}\b")
NGUON = re.compile(r"\[[^\]]+\]\(https?://[^)]+\)|https?://\S+|\(nguồn:[^)]*\)", re.I)


def doc(path: Path) -> str:
    if not path.exists():
        raise SystemExit(f"KHÔNG ĐẠT: không có file {path}")
    text = path.read_text(encoding="utf-8")
    if not text.strip():
        raise SystemExit(f"KHÔNG ĐẠT: {path} rỗng")
    return text


def tieu_de(text: str) -> list[str]:
    """Mọi tiêu đề markdown, đã bỏ dấu # và khoảng trắng."""
    return [l.lstrip("#").strip() for l in text.splitlines() if l.lstrip().startswith("#")]


def dem_tu(text: str) -> int:
    # Bỏ khối code và bảng khỏi phép đếm: chúng không phải phần viết.
    khong_code = re.sub(r"```.*?```", " ", text, flags=re.S)
    khong_bang = "\n".join(l for l in khong_code.splitlines() if not l.strip().startswith("|"))
    return len(khong_bang.split())


def dem_hang_bang(text: str) -> int:
    """Số hàng dữ liệu thật của mọi bảng markdown (bỏ hàng tiêu đề và gạch)."""
    hang = 0
    for l in text.splitlines():
        s = l.strip()
        if s.startswith("|") and s.endswith("|") and not re.fullmatch(r"\|[\s\-:|]+\|", s):
            hang += 1
    # Mỗi bảng có một hàng tiêu đề — trừ đi số bảng.
    so_bang = len(re.findall(r"(?m)^\s*\|[\s\-:|]+\|\s*$", text))
    return max(0, hang - so_bang)


def kiem(
    path: Path,
    muc: list[str],
    tu_toi_thieu: int,
    bang_toi_thieu: int,
    nguon_toi_thieu: int,
    cho_phep_bo_trong: bool,
) -> list[str]:
    text = doc(path)
    loi: list[str] = []

    co = [t.lower() for t in tieu_de(text)]
    for m in muc:
        if not any(m.lower() in t for t in co):
            loi.append(f"thiếu mục '{m}' (tiêu đề đang có: {', '.join(tieu_de(text)) or 'không có'})")

    n = dem_tu(text)
    if n < tu_toi_thieu:
        loi.append(f"mới {n} từ, yêu cầu tối thiểu {tu_toi_thieu}")

    if bang_toi_thieu:
        hang = dem_hang_bang(text)
        if hang < bang_toi_thieu:
            loi.append(f"cần ít nhất {bang_toi_thieu} hàng bảng có dữ liệu, đang có {hang}")

    if nguon_toi_thieu:
        nguon = len(NGUON.findall(text))
        if nguon < nguon_toi_thieu:
            loi.append(f"cần ít nhất {nguon_toi_thieu} nguồn dẫn, đang có {nguon}")

    if not cho_phep_bo_trong:
        thap = text.lower()
        for dau in DAU_HIEU_BO_TRONG:
            if dau in thap:
                loi.append(f"còn chỗ bỏ trống/giữ chỗ: '{dau}'")

    # Số liệu phải có nguồn ở đâu đó trong tài liệu. Kiểm ở mức tài liệu chứ
    # không mức câu: đòi mỗi câu một link sẽ làm tài liệu đọc không nổi.
    than_bai = NGAY_THANG.sub(
        " ", "\n".join(l for l in text.splitlines() if not l.lstrip().startswith(">"))
    )
    if SO_LIEU.search(than_bai) and not NGUON.search(text):
        loi.append("có số liệu nhưng cả tài liệu không có một nguồn nào")

    return loi


def tu_kiem() -> int:
    """Chạy vài ca mẫu để chắc verifier không phải con dấu cao su."""
    import tempfile

    dat = """# Báo cáo
## Mục tiêu
Tối ưu luồng thanh toán trên mobile app cho nhóm khách hàng mới.
## Phát hiện
Theo [báo cáo Baymard](https://baymard.com/lists/cart-abandonment-rate) tỷ lệ
bỏ giỏ hàng trung bình là 70%. Trong 12 phiên usability test, 9 người không
tìm thấy nút quay lại ở bước xác nhận, 4 người hiểu nhầm phí vận chuyển.

| Vấn đề | Mức độ | Số người gặp |
| --- | --- | --- |
| Không thấy nút quay lại | Nghiêm trọng | 9/12 |
| Hiểu nhầm phí ship | Trung bình | 4/12 |
## Đề xuất
Đưa nút quay lại ra thanh trên cố định, hiện phí vận chuyển ngay bước giỏ hàng.
""" + ("thêm chữ cho đủ độ dài. " * 60)

    ca = [
        ("đủ mục và nguồn thì đạt", dat, [], dict()),
        ("thiếu mục thì trượt", dat.replace("## Đề xuất", "## Linh tinh"), ["thiếu mục 'Đề xuất'"], dict()),
        ("còn TODO thì trượt", dat + "\nTODO: bổ sung sau\n", ["còn chỗ bỏ trống"], dict()),
        ("viết ngắn thì trượt", "# Báo cáo\n## Mục tiêu\n## Phát hiện\n## Đề xuất\nxong rồi.\n",
         ["từ, yêu cầu tối thiểu"], dict()),
        ("số liệu không nguồn thì trượt",
         "# B\n## Mục tiêu\nx\n## Phát hiện\nTỷ lệ rời bỏ là 45% theo phân tích của tôi.\n## Đề xuất\ny\n"
         + ("chữ đệm " * 250),
         ["không có một nguồn nào"], dict(bang_toi_thieu=0)),
        ("ngày trong header không bị coi là số liệu bịa",
         "# B\n> Ngày: 2026-09-19 · Trạng thái: nháp\n## Mục tiêu\nx\n## Phát hiện\n"
         "Chín trên mười hai người không tìm thấy nút quay lại.\n\n"
         "| Vấn đề | Số người |\n| --- | --- |\n| Không thấy nút | 9/12 |\n"
         "## Đề xuất\ny\n" + ("chữ đệm " * 250),
         [], dict(nguon_toi_thieu=0)),
        ("bảng rỗng thì trượt", dat.replace("| Không thấy nút quay lại | Nghiêm trọng | 9/12 |", "")
         .replace("| Hiểu nhầm phí ship | Trung bình | 4/12 |", ""),
         ["hàng bảng"], dict(bang_toi_thieu=2)),
    ]
    hong = 0
    with tempfile.TemporaryDirectory() as d:
        for ten, noi_dung, mong_doi, ghi_de in ca:
            p = Path(d) / "t.md"
            p.write_text(noi_dung, encoding="utf-8")
            tham_so = dict(
                muc=["Mục tiêu", "Phát hiện", "Đề xuất"],
                tu_toi_thieu=200,
                bang_toi_thieu=1,
                nguon_toi_thieu=1,
                cho_phep_bo_trong=False,
            )
            tham_so.update(ghi_de)
            loi = kiem(p, **tham_so)
            ok = (not loi) if not mong_doi else any(m in " ".join(loi) for m in mong_doi)
            print(("  ok  " if ok else "TRƯỢT ") + ten + ("" if ok else f" → {loi}"))
            hong += 0 if ok else 1
    print("tự kiểm: " + ("tất cả đúng" if not hong else f"{hong} ca sai"))
    return 1 if hong else 0


def main() -> int:
    ap = argparse.ArgumentParser(description="Kiểm tài liệu UI/UX có đủ chất không.")
    ap.add_argument("file", nargs="?", help="tài liệu markdown cần kiểm")
    ap.add_argument("--muc", action="append", default=[], help="tiêu đề bắt buộc phải có (lặp lại được)")
    ap.add_argument("--tu-toi-thieu", type=int, default=300)
    ap.add_argument("--bang-toi-thieu", type=int, default=0, help="số hàng bảng có dữ liệu tối thiểu")
    ap.add_argument("--nguon-toi-thieu", type=int, default=0, help="số link/nguồn tối thiểu")
    ap.add_argument("--cho-phep-bo-trong", action="store_true", help="bỏ qua kiểm TODO/placeholder")
    ap.add_argument("--tu-kiem", action="store_true", help="chạy ca mẫu kiểm chính script này")
    a = ap.parse_args()

    if a.tu_kiem:
        return tu_kiem()
    if not a.file:
        ap.error("cần đường dẫn tài liệu (hoặc --tu-kiem)")

    loi = kiem(
        Path(a.file),
        a.muc,
        a.tu_toi_thieu,
        a.bang_toi_thieu,
        a.nguon_toi_thieu,
        a.cho_phep_bo_trong,
    )
    if loi:
        print(f"KHÔNG ĐẠT — {a.file}:")
        for l in loi:
            print(f"  · {l}")
        return 1
    print(f"ĐẠT — {a.file}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
