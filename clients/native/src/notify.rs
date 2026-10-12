//! System notifications through `notify-send`; a click on «Открыть» is
//! reported back so the client can show the chat.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

const ACTION: &str = "open";
/// Notifications with a button keep `notify-send` waiting until dismissed;
/// past this many the rest are shown without one.
const WAITING_LIMIT: usize = 4;

static WAITING: AtomicUsize = AtomicUsize::new(0);

/// Shows a notification; the receiver gets a value when the user opens it.
/// Without `notify-send` (or a notification daemon) nothing is shown.
pub fn show(title: &str, body: &str) -> async_channel::Receiver<()> {
    let (opened, receiver) = async_channel::bounded(1);
    let waits = WAITING.fetch_add(1, Ordering::Relaxed) < WAITING_LIMIT;
    if !waits {
        WAITING.fetch_sub(1, Ordering::Relaxed);
    }
    let mut command = Command::new("notify-send");
    command.args(["--app-name=Proteus", "--icon=proteus-native"]);
    if waits {
        command.arg(format!("--action={ACTION}=Открыть"));
    }
    command
        .arg("--hint=string:desktop-entry:proteus-native")
        .args(["--", title, body])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    thread::spawn(move || {
        let output = command.output();
        if waits {
            WAITING.fetch_sub(1, Ordering::Relaxed);
        }
        let Ok(output) = output else {
            return;
        };
        if String::from_utf8_lossy(&output.stdout).trim() == ACTION {
            let _ = opened.try_send(());
        }
    });
    receiver
}
