//! A stdio connection can rotate its idle durable runtime without replacing
//! the OS process. The previous event bridge is joined before shutdown.

use anyhow::{Context, Result};
use proteus_contracts::app_protocol::AppSubagentSession;
use tokio::{sync::mpsc, task::JoinHandle};

use super::super::{AppServerEvent, AppServerHandle, StdioOutput};

pub(super) async fn start_bridge(
    server: &AppServerHandle,
    output: &mpsc::Sender<StdioOutput>,
) -> Result<(JoinHandle<()>, AppSubagentSession)> {
    // Session snapshots can subsume the startup event before the subscriber
    // gets scheduled. Capture that one authoritative event on the raw stream,
    // then let the ordinary session subscription provide snapshots/deltas.
    let mut bootstrap = server.subscribe();
    let mut events = server.subscribe_session();
    server.start_session().await?;
    // start_session awaits publication. Fresh sessions therefore have their
    // event queued now; populated resumed sessions deliberately emit no second
    // SessionStarted and use their existing authoritative runtime identity.
    let identity = AppSubagentSession {
        session_id: server.session_id(),
        session_dir: server
            .session_dir_path()
            .context("stdio session has no durable directory")?,
    };
    loop {
        match bootstrap.try_recv() {
            Ok(event)
                if matches!(&event, AppServerEvent::Runtime { envelope }
                if matches!(envelope.event, crate::domain::Event::SessionStarted { .. })) =>
            {
                output
                    .send(StdioOutput::Event {
                        event: Box::new(event),
                    })
                    .await
                    .context("stdio output closed during session bootstrap")?;
                break;
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(error) => return Err(error).context("stdio session identity stream failed"),
        }
    }
    let output = output.clone();
    let bridge = tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            if matches!(&event, AppServerEvent::Runtime { envelope }
                if matches!(envelope.event, crate::domain::Event::SessionStarted { .. }))
            {
                continue;
            }
            let stop = matches!(event, AppServerEvent::Shutdown);
            if output
                .send(StdioOutput::Event {
                    event: Box::new(event),
                })
                .await
                .is_err()
            {
                break;
            }
            if stop {
                break;
            }
        }
    });
    Ok((bridge, identity))
}

pub(super) async fn stop_bridge(bridge: JoinHandle<()>) {
    bridge.abort();
    let _ = bridge.await;
}
