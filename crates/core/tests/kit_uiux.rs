//! Bộ harness UI/UX (`kits/uiux/`) phải luôn dùng được ngay.
//!
//! Plan trong bộ này là thứ người dùng copy về rồi chạy thẳng. Một khoá gõ sai
//! hay một `deps` trỏ nhầm không phải lỗi chính tả trong tài liệu — nó là một
//! lượt chạy hỏng ở máy người khác.

use agentloom_core::config::Plan;
use std::path::{Path, PathBuf};

fn thu_muc_kit() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../kits/uiux")
}

#[test]
fn moi_plan_trong_bo_uiux_parse_va_validate_duoc() {
    let d = thu_muc_kit().join("plans");
    let mut so = 0;
    for e in std::fs::read_dir(&d)
        .expect("không đọc được kits/uiux/plans")
        .flatten()
    {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "toml") {
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap();
        let plan = Plan::from_toml(&text).unwrap_or_else(|err| panic!("{}: {err}", p.display()));
        plan.validate()
            .unwrap_or_else(|err| panic!("{}: {err}", p.display()));
        assert!(
            !plan.goal.trim().is_empty(),
            "{}: plan thiếu goal",
            p.display()
        );
        so += 1;
    }
    assert_eq!(so, 7, "bộ uiux phải có đủ 7 plan");
}

/// Mỗi node phải có `verify`. Không có thì hệ điều phối chỉ còn tin lời agent —
/// đúng thứ bộ này sinh ra để tránh, vì sản phẩm ở đây là chữ chứ không phải
/// code chạy được hay không.
#[test]
fn moi_node_deu_co_verify_va_goi_dung_verifier() {
    let d = thu_muc_kit().join("plans");
    for e in std::fs::read_dir(&d).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "toml") {
            continue;
        }
        let plan = Plan::from_toml(&std::fs::read_to_string(&p).unwrap()).unwrap();
        for n in &plan.nodes {
            let v = n
                .verify
                .as_ref()
                .unwrap_or_else(|| panic!("{}: node '{}' thiếu verify", p.display(), n.id));
            assert!(
                v.contains("uiux/kiem_tai_lieu.py"),
                "{}: node '{}' phải kiểm bằng verifier của bộ: {v}",
                p.display(),
                n.id
            );
            // Verifier chỉ chặn được bài rỗng nếu có ngưỡng mục bắt buộc.
            assert!(
                v.contains("--muc"),
                "{}: node '{}' phải nêu mục bắt buộc",
                p.display(),
                n.id
            );
        }
    }
}

/// Skill được chèn vào MỌI task, và agentloom chỉ lấy 8 file đầu theo thứ tự
/// tên. Quá số đó là có skill âm thầm không bao giờ tới tay agent.
#[test]
fn so_skill_khong_vuot_qua_gioi_han_duoc_chen() {
    let d = thu_muc_kit().join("skills");
    let so = std::fs::read_dir(&d)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .count();
    assert!(so <= 8, "chỉ 8 skill đầu được chèn, đang có {so}");
}
