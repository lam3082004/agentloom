//! Định danh. NodeId do người/agent đặt nên phải kiểm tra: nó đi vào tên
//! branch git, tên thư mục worktree và tên file skill.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(String);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdError {
    #[error("id rỗng")]
    Empty,
    #[error("id dài quá {0} ký tự")]
    TooLong(usize),
    #[error("id chứa ký tự không hợp lệ: {0:?} (chỉ cho a-z 0-9 - _)")]
    BadChar(char),
    #[error("id không được bắt đầu hoặc kết thúc bằng '-'")]
    BadEdge,
}

const MAX_ID: usize = 64;

impl NodeId {
    /// Chỉ nhận `[a-z0-9_-]`, không rỗng, không mở/đóng bằng `-`.
    /// Đủ hẹp để an toàn khi ghép vào đường dẫn và tên branch git.
    pub fn new(s: impl Into<String>) -> Result<Self, IdError> {
        let s = s.into();
        if s.is_empty() {
            return Err(IdError::Empty);
        }
        if s.len() > MAX_ID {
            return Err(IdError::TooLong(MAX_ID));
        }
        if s.starts_with('-') || s.ends_with('-') {
            return Err(IdError::BadEdge);
        }
        if let Some(c) = s
            .chars()
            .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-' || *c == '_'))
        {
            return Err(IdError::BadChar(c));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Id một lượt chạy. Sinh máy, không nhận từ ngoài.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(String);

impl RunId {
    pub fn generate() -> Self {
        let now = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let tail = uuid::Uuid::new_v4().to_string();
        Self(format!("{now}-{}", &tail[..6]))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nhan_id_hop_le() {
        assert!(NodeId::new("impl-auth").is_ok());
        assert!(NodeId::new("node_1").is_ok());
    }

    #[test]
    fn chan_id_pha_duong_dan() {
        // Đây là lý do NodeId tồn tại: id do agent đặt không được thoát thư mục.
        assert_eq!(NodeId::new("../../etc/passwd"), Err(IdError::BadChar('.')));
        assert_eq!(NodeId::new("a/b"), Err(IdError::BadChar('/')));
        assert_eq!(NodeId::new(""), Err(IdError::Empty));
        assert_eq!(NodeId::new("-x"), Err(IdError::BadEdge));
        assert_eq!(NodeId::new("Impl"), Err(IdError::BadChar('I')));
    }
}
