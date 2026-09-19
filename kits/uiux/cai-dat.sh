#!/usr/bin/env bash
# Cài bộ UI/UX vào một project để agentloom dùng được.
#
# Hai chỗ đến khác nhau, có lý do:
# - skill  → <project>/.agentloom/skills/   (agentloom đọc ở repo gốc, không commit)
# - plan + verifier → <project>/uiux/       (PHẢI commit: worktree của mỗi agent
#   tách ra từ HEAD, nên thứ chưa commit thì agent không nhìn thấy)
set -euo pipefail

nguon="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dich="${1:-}"
tu_commit="no"
[[ "${2:-}" == "--commit" ]] && tu_commit="yes"

if [[ -z "$dich" || "$dich" == "-h" || "$dich" == "--help" ]]; then
	cat <<'HUONG_DAN'
Dùng: cai-dat.sh <thư-mục-project> [--commit]

  <thư-mục-project>  project (git repo) bạn sẽ chạy agentloom trong đó
  --commit           commit luôn uiux/ và dau-vao/ (nếu không, script chỉ in lệnh)

Ví dụ:
  ./cai-dat.sh ~/du-an/ngan-hang-so
  agentloom web --root ~/du-an/ngan-hang-so
HUONG_DAN
	exit 0
fi

[[ -d "$dich" ]] || {
	echo "không có thư mục $dich"
	exit 1
}
dich="$(cd "$dich" && pwd)"

if ! git -C "$dich" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
	echo "! $dich chưa phải git repo."
	echo "  agentloom cần git để cô lập mỗi agent một worktree riêng. Chạy:"
	echo "    cd $dich && git init && git add -A && git commit -m 'khởi tạo'"
	exit 1
fi
# Nằm TRONG một repo khác không đủ: agentloom đặt worktree và branch ở GỐC
# repo, nên cài vào thư mục con của một repo lạ là rải việc của bạn sang repo
# đó. Bắt buộc thư mục project phải chính là gốc repo.
goc="$(git -C "$dich" rev-parse --show-toplevel)"
if [[ "$goc" != "$dich" ]]; then
  echo "! $dich nằm trong repo khác: $goc"
  echo "  agentloom tạo worktree và branch ở gốc repo, nên chạy thế này là đổ"
  echo "  việc sang repo kia. Chọn một trong hai:"
  echo "    · dùng chính gốc repo:   ./cai-dat.sh $goc"
  echo "    · tách project riêng:    cd $dich && git init && git add -A && git commit -m 'khởi tạo'"
  exit 1
fi
if ! git -C "$dich" rev-parse --verify -q HEAD >/dev/null; then
	echo "! repo chưa có commit nào — worktree cần ít nhất một commit. Chạy:"
	echo "    cd $dich && git add -A && git commit -m 'khởi tạo'"
	exit 1
fi

mkdir -p "$dich/.agentloom/skills" "$dich/uiux/plans" "$dich/dau-vao" "$dich/ket-qua"

ghi_de=""
for f in "$nguon"/skills/*.md; do
	ten="$(basename "$f")"
	[[ -e "$dich/.agentloom/skills/$ten" ]] && ghi_de+=" .agentloom/skills/$ten"
done
if [[ -n "$ghi_de" ]]; then
	echo "! đã có sẵn:$ghi_de"
	read -r -p "  ghi đè? [y/N] " tra_loi
	[[ "$tra_loi" == "y" || "$tra_loi" == "Y" ]] || {
		echo "dừng, không đụng gì."
		exit 1
	}
fi

cp "$nguon"/skills/*.md "$dich/.agentloom/skills/"
cp "$nguon"/plans/*.toml "$dich/uiux/plans/"
cp "$nguon"/verify/kiem_tai_lieu.py "$dich/uiux/kiem_tai_lieu.py"
chmod +x "$dich/uiux/kiem_tai_lieu.py"

# Thư mục rỗng không vào được git — mà agent cần thấy chúng tồn tại.
cat >"$dich/dau-vao/DOC-TRUOC.md" <<'GHI_CHU'
# Thư mục dữ liệu đầu vào

Bỏ vào đây mọi thứ agent cần đọc: transcript phỏng vấn, kết quả khảo sát (csv),
ghi chép usability test, mô tả màn hình, link Figma, tài liệu sản phẩm.

Agent CHỈ ĐỌC thư mục này, không sửa. Nhớ `git add dau-vao && git commit` trước
khi chạy: mỗi agent làm việc trong một git worktree tách từ HEAD, file chưa
commit thì agent không nhìn thấy.
GHI_CHU
[[ -e "$dich/ket-qua/.gitkeep" ]] || : >"$dich/ket-qua/.gitkeep"

echo "đã cài vào $dich:"
echo "  .agentloom/skills/   $(ls "$nguon"/skills | wc -l) skill (agentloom tự chèn vào mọi agent)"
echo "  uiux/plans/          $(ls "$nguon"/plans | wc -l) plan"
echo "  uiux/kiem_tai_lieu.py  verifier tài liệu"
echo "  dau-vao/  ket-qua/"

if [[ "$tu_commit" == "yes" ]]; then
	git -C "$dich" add uiux dau-vao ket-qua
	git -C "$dich" commit -qm "thêm bộ harness UI/UX cho agentloom" || echo "(không có gì mới để commit)"
	echo "đã commit uiux/, dau-vao/, ket-qua/."
else
	echo
	echo "BƯỚC BẮT BUỘC TIẾP THEO — agent chỉ thấy file đã commit:"
	echo "    cd $dich && git add uiux dau-vao ket-qua && git commit -m 'thêm bộ harness UI/UX'"
fi
echo
echo "Chạy thử:  agentloom run uiux/plans/2-danh-gia-thiet-ke.toml --root $dich --web"
