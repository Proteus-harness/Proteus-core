//! Headless terminal: a shell on a PTY parsed by `alacritty_terminal`.
//!
//! The UI only sees [`Terminal`] (input, resize, scroll) and immutable
//! [`Screen`] snapshots; emulation, PTY I/O and parsing run on alacritty's
//! own thread.

mod keys;
mod screen;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use anyhow::{Context as _, Result};

pub use keys::{Key, encode};
pub use screen::{Base, Run, Screen};

/// What the terminal reports back to its view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    Title(Option<String>),
    Bell,
    /// The shell exited; the terminal is dead.
    Exited,
}

#[derive(Clone)]
struct Listener {
    signals: async_channel::Sender<Signal>,
    redraw: async_channel::Sender<()>,
    pty: Arc<OnceLock<EventLoopSender>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let signal = match event {
            // Coalesced: one pending redraw is enough however much arrives.
            Event::Wakeup => {
                let _ = self.redraw.try_send(());
                return;
            }
            Event::PtyWrite(text) => {
                if let Some(pty) = self.pty.get() {
                    let _ = pty.send(Msg::Input(Cow::Owned(text.into_bytes())));
                }
                return;
            }
            Event::Title(title) => Signal::Title(Some(title)),
            Event::ResetTitle => Signal::Title(None),
            Event::Bell => Signal::Bell,
            Event::Exit | Event::ChildExit(_) => Signal::Exited,
            _ => return,
        };
        let _ = self.signals.try_send(signal);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub columns: usize,
    pub rows: usize,
}

impl Grid {
    pub fn new(columns: usize, rows: usize) -> Self {
        Self {
            columns: columns.max(2),
            rows: rows.max(1),
        }
    }

    fn window(self) -> WindowSize {
        WindowSize {
            num_lines: self.rows.min(u16::MAX as usize) as u16,
            num_cols: self.columns.min(u16::MAX as usize) as u16,
            cell_width: 1,
            cell_height: 1,
        }
    }
}

impl Dimensions for Grid {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: EventLoopSender,
    grid: Grid,
}

pub struct Spawned {
    pub terminal: Terminal,
    pub signals: async_channel::Receiver<Signal>,
    pub redraw: async_channel::Receiver<()>,
}

impl Terminal {
    /// Starts the user's login shell (`$SHELL`) in `cwd`.
    pub fn spawn(cwd: &Path, grid: Grid) -> Result<Spawned> {
        let (signal_tx, signals) = async_channel::unbounded();
        let (redraw_tx, redraw) = async_channel::bounded(1);
        let shared = Arc::new(OnceLock::new());
        let listener = Listener {
            signals: signal_tx,
            redraw: redraw_tx,
            pty: shared.clone(),
        };
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &grid, listener.clone())));
        let options = tty::Options {
            shell: None,
            working_directory: Some(cwd.to_path_buf()),
            drain_on_exit: false,
            env: HashMap::from([
                ("TERM".to_owned(), "xterm-256color".to_owned()),
                ("COLORTERM".to_owned(), "truecolor".to_owned()),
            ]),
        };
        let pty = tty::new(&options, grid.window(), 0).context("не удалось открыть PTY")?;
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)
            .context("не удалось запустить терминал")?;
        let sender = event_loop.channel();
        let _ = shared.set(sender.clone());
        event_loop.spawn();
        Ok(Spawned {
            terminal: Self {
                term,
                pty: sender,
                grid,
            },
            signals,
            redraw,
        })
    }

    /// Sends keyboard input; typing always returns to the live screen.
    pub fn write(&self, bytes: Vec<u8>) {
        self.term.lock().scroll_display(Scroll::Bottom);
        let _ = self.pty.send(Msg::Input(Cow::Owned(bytes)));
    }

    pub fn paste(&self, text: &str) {
        let bracketed = self.mode().contains(TermMode::BRACKETED_PASTE);
        self.write(paste_bytes(text, bracketed));
    }

    pub fn app_cursor(&self) -> bool {
        self.mode().contains(TermMode::APP_CURSOR)
    }

    fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    pub fn resize(&mut self, grid: Grid) {
        if grid == self.grid {
            return;
        }
        self.grid = grid;
        self.term.lock().resize(grid);
        let _ = self.pty.send(Msg::Resize(grid.window()));
    }

    /// Scrolls the history; positive moves up.
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    pub fn snapshot(&self, base: Base) -> Screen {
        screen::snapshot(&self.term.lock(), base)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.pty.send(Msg::Shutdown);
    }
}

fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // A pasted end marker would let the text escape bracketed mode.
        let text = text.replace("\x1b[201~", "");
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn paste_uses_carriage_returns_and_brackets() {
        assert_eq!(paste_bytes("a\nb\r\nc", false), b"a\rb\rc");
        assert_eq!(
            paste_bytes("x\x1b[201~y", true),
            b"\x1b[200~xy\x1b[201~".to_vec()
        );
    }

    #[test]
    fn shell_output_reaches_the_screen() {
        let dir = std::env::temp_dir();
        let spawned = Terminal::spawn(&dir, Grid::new(40, 6)).expect("spawn");
        spawned
            .terminal
            .write(b"printf 'proteus-%s\\n' terminal\r".to_vec());
        let base = Base {
            foreground: [200, 200, 200],
            background: [0, 0, 0],
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let screen = spawned.terminal.snapshot(base);
            if screen.text().contains("proteus-terminal") {
                break;
            }
            assert!(Instant::now() < deadline, "no output:\n{}", screen.text());
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
