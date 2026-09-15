//! Cô lập workspace bằng git worktree.
//!
//! Mỗi node chạy song song cần cây thư mục riêng, nếu không hai agent sửa
//! cùng file sẽ đạp lên nhau. Worktree rẻ (tạo trong mili-giây, chung object
//! store) và cho ta thêm một thứ quý: diff của từng agent nằm trên branch
//! riêng, review được tách bạch.
//!
//! Khi thư mục không phải git repo, ta không giả vờ cô lập — mọi node dùng
//! chung thư mục gốc và chuyện đó được ghi vào event log.

use crate::ids::NodeId;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct Worktrees {
    repo_root: PathBuf,
    base: PathBuf,
    enabled: bool,
    /// Worktree và branch đều mang tên run: chạy lại trên cùng repo không
    /// đụng phải branch còn sót của lần trước.
    run: String,
}

async fn git(dir: &Path, args: &[&str]) -> anyhow::Result<(bool, String)> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .await?;
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), s.trim().to_string()))
}

impl Worktrees {
    /// `enabled` tắt khi thư mục không phải git repo.
    pub async fn discover(root: impl AsRef<Path>, run: &str) -> Self {
        let root = root.as_ref().to_path_buf();
        let is_repo = git(&root, &["rev-parse", "--is-inside-work-tree"])
            .await
            .map(|(ok, out)| ok && out == "true")
            .unwrap_or(false);
        let repo_root = if is_repo {
            git(&root, &["rev-parse", "--show-toplevel"])
                .await
                .ok()
                .filter(|(ok, _)| *ok)
                .map(|(_, p)| PathBuf::from(p))
                .unwrap_or_else(|| root.clone())
        } else {
            root.clone()
        };
        if is_repo {
            // Dọn đăng ký worktree trỏ vào thư mục đã bị xoá tay.
            let _ = git(&repo_root, &["worktree", "prune"]).await;
        }
        let base = repo_root.join(".agentgraph").join("worktrees").join(run);
        Self {
            repo_root,
            base,
            enabled: is_repo,
            run: run.to_string(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }
    pub fn branch_of(&self, id: &NodeId) -> String {
        format!("ag/{}/{id}", self.run)
    }

    /// Tạo worktree cho node. Trả về thư mục agent sẽ chạy trong đó.
    /// Không phải repo git thì trả về thư mục gốc — cô lập bị tắt, có chủ ý.
    pub async fn create(&self, id: &NodeId) -> anyhow::Result<PathBuf> {
        if !self.enabled {
            return Ok(self.repo_root.clone());
        }
        let path = self.base.join(id.as_str());
        if path.exists() {
            return Ok(path);
        }
        tokio::fs::create_dir_all(&self.base).await?;
        let branch = self.branch_of(id);
        let p = path.to_string_lossy().into_owned();
        // Branch có thể còn sót từ lần chạy trước; thử tạo mới, không được thì bám vào branch cũ.
        let (ok, err) = git(
            &self.repo_root,
            &["worktree", "add", "-b", &branch, &p, "HEAD"],
        )
        .await?;
        if !ok {
            let (ok2, err2) = git(&self.repo_root, &["worktree", "add", &p, &branch]).await?;
            if !ok2 {
                anyhow::bail!("không tạo được worktree cho {id}: {err} / {err2}");
            }
        }
        Ok(path)
    }

    /// Gỡ worktree. `keep_branch` để giữ lại công việc của agent mà review sau.
    pub async fn remove(&self, id: &NodeId, keep_branch: bool) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let p = self.base.join(id.as_str()).to_string_lossy().into_owned();
        let _ = git(&self.repo_root, &["worktree", "remove", "--force", &p]).await;
        if !keep_branch {
            let _ = git(&self.repo_root, &["branch", "-D", &self.branch_of(id)]).await;
        }
        Ok(())
    }

    /// Có thay đổi nào chưa commit trong worktree của node không.
    pub async fn has_changes(&self, id: &NodeId) -> bool {
        if !self.enabled {
            return false;
        }
        let p = self.base.join(id.as_str());
        match git(&p, &["status", "--porcelain"]).await {
            Ok((true, s)) => !s.is_empty(),
            _ => false,
        }
    }

    /// Commit mọi thay đổi agent để lại, để join có cái mà merge.
    pub async fn commit_all(&self, id: &NodeId, msg: &str) -> anyhow::Result<bool> {
        if !self.enabled {
            return Ok(false);
        }
        let p = self.base.join(id.as_str());
        // Loại .agentgraph: nếu commit, file mutation sẽ theo branch merge
        // sang node join và bị áp lại lần nữa.
        let (_, _) = git(&p, &["add", "-A", "--", ".", ":!.agentgraph"]).await?;
        let (ok, _) = git(&p, &["commit", "-m", msg]).await?;
        Ok(ok)
    }

    /// Merge branch của các node nguồn vào worktree của node đích.
    /// Trả về danh sách branch merge không sạch — join phải biết để xử lý.
    pub async fn merge_into(
        &self,
        target: &NodeId,
        sources: &[NodeId],
    ) -> anyhow::Result<Vec<String>> {
        if !self.enabled {
            return Ok(Vec::new());
        }
        let p = self.base.join(target.as_str());
        let mut conflicts = Vec::new();
        for s in sources {
            let b = self.branch_of(s);
            let (ok, _) = git(&p, &["merge", "--no-edit", "--no-ff", &b]).await?;
            if !ok {
                let _ = git(&p, &["merge", "--abort"]).await;
                conflicts.push(b);
            }
        }
        Ok(conflicts)
    }
}
