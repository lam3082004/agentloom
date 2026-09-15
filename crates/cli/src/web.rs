//! Mặt web: cùng một `View` với TUI, chỉ khác phần vẽ.
//!
//! Trình duyệt nạp ảnh chụp trạng thái một lần qua `/api/view`, rồi nghe
//! `/api/events` (SSE) để cập nhật. Nhờ đó mở tab muộn vẫn thấy đầy đủ, và
//! reload không mất gì — khác với thiết kế chỉ-stream.
//!
//! Quan trọng: SSE đẩy `Patch` **đã fold sẵn**, không đẩy event thô. Client
//! chỉ ghép dữ liệu, không diễn giải event. Nếu để client tự fold thì sẽ có
//! hai bản cùng một quy tắc — một Rust một JS — và chúng sẽ lệch nhau.

use agentgraph_core::event::EventLog;
use agentgraph_core::view::{Patch, View};
use axum::extract::State;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::{Json, Router};
use std::convert::Infallible;
use std::sync::{Arc, RwLock};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

#[derive(Clone)]
pub struct WebState {
    view: Arc<RwLock<View>>,
    /// `None` ở chế độ xem lại: không còn gì phát thêm.
    patches: Option<tokio::sync::broadcast::Sender<Patch>>,
}

impl WebState {
    /// Chế độ live: fold event vào view ở nền, đồng thời phát cho trình duyệt.
    pub fn live(log: EventLog) -> Self {
        let view = Arc::new(RwLock::new(View::default()));
        let (tx, _) = tokio::sync::broadcast::channel(4096);
        let v = view.clone();
        let tx2 = tx.clone();
        let mut rx = log.subscribe();
        tokio::spawn(async move {
            while let Ok(ev) = rx.recv().await {
                let patch = {
                    let Ok(mut g) = v.write() else { continue };
                    let (node, lines) = g.apply_tracked(&ev);
                    Patch {
                        view: g.meta(),
                        node,
                        lines,
                    }
                };
                let _ = tx2.send(patch);
            }
        });
        Self {
            view,
            patches: Some(tx),
        }
    }

    /// Chế độ xem lại: view đã dựng sẵn, không có gì phát thêm.
    pub fn replay(view: View) -> Self {
        Self {
            view: Arc::new(RwLock::new(view)),
            patches: None,
        }
    }
}

pub fn router(state: WebState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/view", get(api_view))
        .route("/api/events", get(api_events))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../assets/index.html"))
}

async fn api_view(State(s): State<WebState>) -> impl IntoResponse {
    let v = s.view.read().map(|g| g.clone()).unwrap_or_default();
    Json(v)
}

async fn api_events(
    State(s): State<WebState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    // Không có log (chế độ replay) thì trả một stream rỗng thay vì lỗi:
    // trình duyệt đã có toàn bộ dữ liệu từ /api/view rồi.
    let stream = match &s.patches {
        Some(tx) => {
            let rx = BroadcastStream::new(tx.subscribe());
            futures_either::Either::Left(rx.filter_map(|r| {
                r.ok().and_then(|p: Patch| {
                    serde_json::to_string(&p)
                        .ok()
                        .map(|j| Ok(SseEvent::default().data(j)))
                })
            }))
        }
        None => futures_either::Either::Right(tokio_stream::pending()),
    };
    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

/// `Either` tối giản để hai nhánh trả về cùng một kiểu stream mà không phải
/// kéo thêm `futures` vào chỉ vì một chỗ.
mod futures_either {
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio_stream::Stream;

    pub enum Either<L, R> {
        Left(L),
        Right(R),
    }

    impl<L, R, T> Stream for Either<L, R>
    where
        L: Stream<Item = T> + Unpin,
        R: Stream<Item = T> + Unpin,
    {
        type Item = T;
        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
            match self.get_mut() {
                Either::Left(l) => Pin::new(l).poll_next(cx),
                Either::Right(r) => Pin::new(r).poll_next(cx),
            }
        }
    }
}

pub async fn serve(state: WebState, port: u16) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| anyhow::anyhow!("không mở được cổng {port}: {e} — thử --port khác"))?;
    let addr = listener.local_addr()?;
    println!("web đang chạy: http://{addr}");
    axum::serve(listener, router(state)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentgraph_core::event::{EventKind, Origin};
    use agentgraph_core::ids::{NodeId, RunId};

    fn tmp_log() -> (EventLog, std::path::PathBuf) {
        let d = std::env::temp_dir().join(format!("ag-web-{}", uuid::Uuid::new_v4()));
        (EventLog::create(d.join("e.jsonl")).unwrap(), d)
    }

    #[tokio::test]
    async fn live_fold_roi_phat_patch_da_gop_san() {
        let (log, dir) = tmp_log();
        let state = WebState::live(log.clone());
        let mut rx = state.patches.as_ref().unwrap().subscribe();

        log.emit(
            None,
            EventKind::RunStarted {
                goal: "mục tiêu".into(),
                run: RunId::generate(),
            },
        );
        let p = rx.recv().await.unwrap();
        assert_eq!(p.view.goal, "mục tiêu");

        let n = NodeId::new("a").unwrap();
        log.emit(
            Some(n.clone()),
            EventKind::NodeAdded {
                title: "A".into(),
                agent: "claude".into(),
                deps: vec![],
                by: Origin::Plan,
            },
        );
        let p = rx.recv().await.unwrap();
        assert_eq!(p.view.nodes.len(), 1);
        assert_eq!(p.view.nodes[0].agent, "claude");

        log.emit(
            Some(n.clone()),
            EventKind::AgentTool {
                name: "Edit".into(),
                detail: "src/api.rs".into(),
            },
        );
        let p = rx.recv().await.unwrap();
        // Client chỉ nối chuỗi này vào, không phải tự hiểu event là gì.
        assert_eq!(p.node.as_deref(), Some("a"));
        assert_eq!(p.lines, vec!["→ Edit src/api.rs".to_string()]);

        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn patch_khong_mang_theo_log_de_khoi_phinh() {
        let (log, dir) = tmp_log();
        let state = WebState::live(log.clone());
        let mut rx = state.patches.as_ref().unwrap().subscribe();
        let n = NodeId::new("a").unwrap();
        log.emit(
            Some(n.clone()),
            EventKind::NodeAdded {
                title: "A".into(),
                agent: "fake".into(),
                deps: vec![],
                by: Origin::Plan,
            },
        );
        let _ = rx.recv().await.unwrap();
        log.emit(
            Some(n),
            EventKind::AgentText {
                text: "CHUOI_RAT_DAI_KHONG_DUOC_LAP_LAI".into(),
            },
        );
        let p = rx.recv().await.unwrap();
        let meta = serde_json::to_string(&p.view).unwrap();
        assert!(
            !meta.contains("CHUOI_RAT_DAI"),
            "meta phải sạch log, nếu không mỗi event sẽ đẩy lại toàn bộ lịch sử"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn che_do_xem_lai_khong_co_kenh_phat() {
        let state = WebState::replay(View::default());
        assert!(state.patches.is_none(), "replay không được giả vờ còn live");
    }
}
