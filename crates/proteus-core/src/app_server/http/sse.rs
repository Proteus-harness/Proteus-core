use std::{convert::Infallible, time::Duration};

use async_stream::stream;
use bytes::Bytes;
use http_body_util::{BodyExt, StreamBody};
use hyper::{
    Response, StatusCode,
    body::Frame,
    header::{CACHE_CONTROL, CONNECTION, CONTENT_TYPE},
};

use super::{HttpAppState, HttpResponse, command_response};
use crate::app_server::{AppServerEvent, StdioOutput};

const SSE_HEARTBEAT_SECS: u64 = 15;

pub(super) async fn sse_response(
    state: HttpAppState,
    server: crate::app_server::AppServerHandle,
) -> HttpResponse {
    let mut events = server.subscribe_session();
    let mut activity_events = state.subscribe_activity();
    #[cfg(unix)]
    let mut catalog_updates = crate::core::subscribe_catalog_updates();
    #[cfg(not(unix))]
    let mut catalog_updates = tokio::sync::watch::channel(0_u64).1;
    let body = StreamBody::new(stream! {
        yield Ok::<Frame<Bytes>, Infallible>(Frame::data(Bytes::from_static(b": connected\n\n")));

        if let Err(error) = server.start_session().await {
            let output = command_response(None, Err(error));
            yield Ok(Frame::data(encode_sse_output(&output)));
            return;
        }
        state.emit_session_activity_for_server(&server).await;

        // /sessions can finish refreshing before this connection exists.
        // Reconcile that list once, including after reconnect. Later changes
        // remain pending for the watch select below.
        let catalog_was_refreshed = *catalog_updates.borrow_and_update() != 0;
        if catalog_was_refreshed {
            yield Ok(Frame::data(encode_sse_output(&StdioOutput::Event {
                event: Box::new(AppServerEvent::SessionCatalogUpdated),
            })));
        }

        let mut heartbeat = tokio::time::interval(Duration::from_secs(SSE_HEARTBEAT_SECS));
        let mut catalog_open = true;
        loop {
            tokio::select! {
                changed = catalog_updates.changed(), if catalog_open => {
                    if changed.is_err() {
                        catalog_open = false;
                        continue;
                    }
                    yield Ok(Frame::data(encode_sse_output(&StdioOutput::Event {
                        event: Box::new(AppServerEvent::SessionCatalogUpdated),
                    })));
                }
                _ = heartbeat.tick() => {
                    yield Ok(Frame::data(Bytes::from_static(b": keep-alive\n\n")));
                }
                event = events.recv() => {
                    match event {
                        Ok(event) => {
                            let should_stop = matches!(event, AppServerEvent::Shutdown);
                            let output = StdioOutput::Event {
                                event: Box::new(event),
                            };
                            yield Ok(Frame::data(encode_sse_output(&output)));
                            if should_stop {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                event = activity_events.recv() => {
                    match event {
                        Ok(event) => {
                            let output = StdioOutput::Event {
                                event: Box::new(event),
                            };
                            yield Ok(Frame::data(encode_sse_output(&output)));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                            events.request_snapshot();
                            let output = StdioOutput::Event {
                                event: Box::new(AppServerEvent::EventStreamLagged { count }),
                            };
                            yield Ok(Frame::data(encode_sse_output(&output)));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    })
    .boxed_unsync();

    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .header(CACHE_CONTROL, "no-cache")
        .header(CONNECTION, "keep-alive")
        .body(body)
        .expect("sse response is valid")
}

pub(super) fn encode_sse_output(output: &StdioOutput) -> Bytes {
    let data = serde_json::to_string(output).unwrap_or_else(|error| {
        serde_json::to_string(&serde_json::json!({
            "type": "response",
            "id": null,
            "ok": false,
            "output": null,
            "error": format!("{error:#}"),
        }))
        .expect("fallback response serializes")
    });
    Bytes::from(format!("event: output\ndata: {data}\n\n"))
}
