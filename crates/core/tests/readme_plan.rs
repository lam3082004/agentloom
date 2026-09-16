//! Mọi plan mẫu trong README phải thật sự chạy được.
//!
//! `Plan`/`NodeSpec` dùng `deny_unknown_fields` và `validate` chặn plan rỗng:
//! kể từ đó, một khoá gõ nhầm trong tài liệu không còn là lỗi chính tả vô hại
//! mà là một plan người đọc copy về sẽ không chạy nổi.
use agentloom_core::config::Plan;
use std::path::Path;

#[test]
fn readme_toml_blocks_parse_va_validate() {
    let readme =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md"))
            .unwrap();
    let mut blocks = Vec::new();
    let mut cur: Option<String> = None;
    for line in readme.lines() {
        if line.trim_start().starts_with("```toml") {
            cur = Some(String::new());
        } else if line.trim() == "```" {
            if let Some(b) = cur.take() {
                blocks.push(b);
            }
        } else if let Some(b) = cur.as_mut() {
            b.push_str(line);
            b.push('\n');
        }
    }
    assert!(
        !blocks.is_empty(),
        "không tìm thấy khối toml nào trong README"
    );
    let mut loi = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        // Mục "4. Viết plan" là bảng tham chiếu từng field, cố ý trỏ
        // `deps = ["node-khac"]` tới một node không có thật để minh hoạ cú
        // pháp. Nhận ra nó bằng NỘI DUNG, không bằng số thứ tự: thêm một khối
        // toml vào README là số thứ tự lệch hết và test bỏ qua nhầm khối.
        if b.contains("node-khac") {
            continue;
        }
        match Plan::from_toml(b) {
            Ok(p) => {
                if let Err(e) = p.validate() {
                    loi.push(format!("khối {}: validate lỗi: {e}\n---\n{b}", i + 1));
                }
            }
            Err(e) => loi.push(format!("khối {}: parse lỗi: {e}\n---\n{b}", i + 1)),
        }
    }
    assert!(
        loi.is_empty(),
        "{} khối lỗi:\n{}",
        loi.len(),
        loi.join("\n\n")
    );
}
