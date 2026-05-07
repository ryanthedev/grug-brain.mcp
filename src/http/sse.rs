//! Server-Sent Events stream of `MemoryEvent`s.

use super::AppState;
use crate::types::MemoryEvent;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;

pub async fn events(
    State(state): State<Arc<AppState>>,
) -> Sse<Box<dyn Stream<Item = Result<Event, Infallible>> + Send + Unpin>> {
    let stream: Box<dyn Stream<Item = Result<Event, Infallible>> + Send + Unpin> =
        match state.events.as_ref() {
            None => {
                // No watcher: emit a single "ready" comment then idle.
                let initial = stream::iter(vec![Ok(Event::default().comment("watcher disabled"))]);
                let pending = stream::pending();
                Box::new(initial.chain(pending))
            }
            Some(sender) => {
                let rx = sender.subscribe();
                let s = BroadcastStream::new(rx).filter_map(|res| async move {
                    match res {
                        Ok(evt) => Some(Ok::<_, Infallible>(
                            Event::default().event("memory").data(serialize_event(&evt)),
                        )),
                        Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                            Some(Ok(Event::default()
                                .event("lagged")
                                .data(json!({"lagged": n}).to_string())))
                        }
                    }
                });
                Box::new(Box::pin(s))
            }
        };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keep-alive"),
    )
}

fn serialize_event(evt: &MemoryEvent) -> String {
    match evt {
        MemoryEvent::Created { brain, path, mtime } => {
            json!({"kind": "created", "brain": brain, "path": path, "mtime": mtime}).to_string()
        }
        MemoryEvent::Modified { brain, path, mtime } => {
            json!({"kind": "modified", "brain": brain, "path": path, "mtime": mtime}).to_string()
        }
        MemoryEvent::Deleted { brain, path } => {
            json!({"kind": "deleted", "brain": brain, "path": path}).to_string()
        }
        MemoryEvent::Renamed {
            brain,
            from,
            to,
            mtime,
        } => json!({
            "kind": "renamed",
            "brain": brain,
            "from": from,
            "to": to,
            "mtime": mtime,
        })
        .to_string(),
        MemoryEvent::Reload { brain, paths, reason } => json!({
            "kind": "reload",
            "brain": brain,
            "paths": paths,
            "reason": reason,
        })
        .to_string(),
        MemoryEvent::Lagged(n) => json!({"kind": "lagged", "n": n}).to_string(),
    }
}
