//! Terminal lifecycle and the input thread.
//!
//! [`TerminalGuard`] enters raw mode and the alternate screen (through
//! `ratatui::try_init`, which also installs a panic hook that restores the
//! terminal) and restores everything when dropped, on every exit path. An
//! extra panic hook makes the cursor visible again before the panic message
//! is printed.
//!
//! [`InputThread`] reads crossterm events on a background thread and forwards
//! them over an `mpsc` channel. It polls with a short timeout so it can be
//! stopped and joined cleanly instead of being left blocked in `read()`.

use std::io::{self, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::DefaultTerminal;

/// Restores the terminal when dropped.
pub struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    /// Enters raw mode and the alternate screen and returns the terminal.
    pub fn enter() -> io::Result<(TerminalGuard, DefaultTerminal)> {
        let terminal = ratatui::try_init()?;
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(stdout(), Show);
            previous(info);
        }));
        Ok((TerminalGuard { active: true }, terminal))
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            let _ = execute!(stdout(), Show);
            ratatui::restore();
        }
    }
}

/// The background reader of terminal events.
pub struct InputThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl InputThread {
    /// Starts the reader. Every event (or read error) is sent to the returned receiver.
    pub fn start() -> (InputThread, Receiver<io::Result<Event>>) {
        let (tx, rx): (Sender<io::Result<Event>>, Receiver<io::Result<Event>>) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                match event::poll(Duration::from_millis(100)) {
                    Ok(true) => {
                        let ev = event::read();
                        let failed = ev.is_err();
                        if tx.send(ev).is_err() || failed {
                            break;
                        }
                    }
                    Ok(false) => {}
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            }
        });
        (
            InputThread {
                stop,
                handle: Some(handle),
            },
            rx,
        )
    }
}

impl Drop for InputThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
