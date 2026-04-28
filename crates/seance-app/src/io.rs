//! PTY reader thread.
//!
//! Owns the blocking `read()` on the master PTY and forwards bytes to the
//! UI thread via the winit `EventLoopProxy`. Without this split the event
//! loop has to wake periodically just to drain the PTY, which is the
//! 250 Hz poll the deadline scheduler (#24) is replacing.

use std::io::Read;

use winit::event_loop::EventLoopProxy;

use seance_vt::Terminal;

use crate::UserEvent;

/// Spawn a thread that drains the PTY reader and forwards every chunk to
/// the UI via `proxy`. The thread exits on EOF or an unrecoverable read
/// error after sending [`UserEvent::PtyExited`]; if the proxy is dead
/// (event loop already gone) it exits silently.
pub(crate) fn spawn_pty_reader(mut reader: Box<dyn Read + Send>, proxy: EventLoopProxy<UserEvent>) {
    std::thread::Builder::new()
        .name("seance-pty-reader".into())
        .spawn(move || {
            let mut buf = vec![0u8; Terminal::READ_CHUNK_SIZE];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        let _ = proxy.send_event(UserEvent::PtyExited);
                        return;
                    }
                    Ok(n) => {
                        let chunk = buf[..n].to_vec();
                        if proxy.send_event(UserEvent::PtyData(chunk)).is_err() {
                            return;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        let _ = proxy.send_event(UserEvent::PtyExited);
                        return;
                    }
                }
            }
        })
        .expect("failed to spawn pty reader thread");
}
