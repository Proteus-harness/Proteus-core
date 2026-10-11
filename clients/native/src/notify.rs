//! System notifications through `notify-send`; a click on «Открыть» is
//! reported back so the client can show the chat.

use std::process::{Command, Stdio};
use std::thread;

const ACTION: &str = "open";

/// Shows a notification; the receiver gets a value when the user opens it.
/// Without `notify-send` (or a notification daemon) nothing is shown.
pub fn show(title: &str, body: &str) -> async_channel::Receiver<()> {
    let (opened, receiver) = async_channel::bounded(1);
    let mut command = Command::new("notify-send");
    command
        .args(["--app-name=Proteus", "--icon=proteus-native"])
        .arg(format!("--action={ACTION}=Открыть"))
        .arg("--hint=string:desktop-entry:proteus-native")
        .args(["--", title, body])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    thread::spawn(move || {
        let Ok(output) = command.output() else {
            return;
        };
        if String::from_utf8_lossy(&output.stdout).trim() == ACTION {
            let _ = opened.try_send(());
        }
    });
    receiver
}
