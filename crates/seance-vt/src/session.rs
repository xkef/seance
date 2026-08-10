//! Winit-free VT/PTY actor session API.
//!
//! The actor owns libghostty and the PTY on one Unix IO thread. UI code sends
//! commands and consumes immutable [`VtSnapshot`] values from [`SnapshotSlot`].

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bytes::{Buf, Bytes};

pub use seance_protocol::frame::{Resize, ThemeColors};

use crate::clipboard::ClipboardRequest;
use crate::core::VtCoreError;
use crate::{CursorShape, VtSnapshot};

const SYNC_OUTPUT_TIMEOUT: Duration = Duration::from_millis(150);

/// Coalesce window handed to actors constructed from a defaulted
/// [`VtSessionOptions`]. Mirrors `seance_config`'s `io.coalesce_delay_ms`
/// default.
const DEFAULT_COALESCE_DELAY_MS: u16 = 2;

#[cfg(unix)]
const READ_CHUNK: usize = 16 * 1024;
#[cfg(unix)]
const MAX_READ_PER_TICK: usize = 256 * 1024;
#[cfg(unix)]
const PTY_KEY: usize = 0;
/// Lower bound on the settle window applied once per `read_pty_batch` after the
/// PTY drains. A typical shell emits a clear-screen and the prompt redraw as two
/// adjacent writes; in release builds the actor often drains the first before the
/// second lands, which paints the cleared-with-home-cursor frame for one vsync.
/// Polling once for ~1 ms after `WouldBlock` lets that second write coalesce into
/// the same snapshot. The configurable `coalesce_delay_ms` widens this window for
/// bursty output; a recent keystroke caps it back to this floor so echo latency
/// is unaffected.
#[cfg(unix)]
const PTY_READ_SETTLE: Duration = Duration::from_millis(1);
/// A UI write (keystroke or paste) within this window marks the actor as
/// echo-sensitive, capping the settle window to [`PTY_READ_SETTLE`].
#[cfg(unix)]
const INPUT_ACTIVITY_WINDOW: Duration = Duration::from_millis(50);

/// Settle window applied after a drained PTY read. `coalesce_delay` is the
/// configured `io.coalesce_delay_ms`; `recent_input` caps it to
/// [`PTY_READ_SETTLE`] so a keystroke echo is not delayed by a large delay. A
/// zero window disables the settle entirely (restoring the un-coalesced read
/// loop).
#[cfg(unix)]
fn settle_window(coalesce_delay: Duration, recent_input: bool) -> Duration {
    if recent_input {
        coalesce_delay.min(PTY_READ_SETTLE)
    } else {
        coalesce_delay
    }
}

/// Options used when spawning a VT session actor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VtSessionOptions {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
    pub initial_cursor_shape: CursorShape,
    pub max_scrollback: usize,
    /// PTY-output coalesce window in milliseconds. See `PTY_READ_SETTLE` /
    /// `settle_window` for how the actor applies it.
    pub coalesce_delay_ms: u16,
}

impl Default for VtSessionOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            pixel_width: 800,
            pixel_height: 384,
            initial_cursor_shape: CursorShape::Block,
            max_scrollback: crate::core::DEFAULT_MAX_SCROLLBACK,
            coalesce_delay_ms: DEFAULT_COALESCE_DELAY_MS,
        }
    }
}

/// Public events emitted by the VT actor.
///
/// All variants are pure wake-up signals — actual payloads (snapshots,
/// clipboard requests) live in side-channels so the event itself can stay
/// `Copy + Eq` for cheap deduplication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VtEvent {
    ContentDirty,
    /// One or more OSC 52 clipboard requests have been queued by the VT
    /// thread. The consumer should drain them via
    /// [`VtSessionHandle::drain_clipboard_requests`].
    ClipboardActivity,
    Exited,
}

/// Commands accepted by the VT actor.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VtCommand {
    Write(Bytes),
    Resize {
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    },
    SetThemeColors(ThemeColors),
    ScrollLines(i32),
    SetCursorShape(CursorShape),
    AckRendered(u64),
    Shutdown,
}

/// Latest immutable snapshot slot shared between actor and UI.
#[derive(Clone, Debug, Default)]
pub struct SnapshotSlot {
    inner: Arc<Mutex<Option<Arc<VtSnapshot>>>>,
}

impl SnapshotSlot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn latest_snapshot(&self) -> Option<Arc<VtSnapshot>> {
        self.inner
            .lock()
            .expect("snapshot slot mutex poisoned")
            .clone()
    }

    pub(crate) fn publish(&self, snapshot: Arc<VtSnapshot>) {
        *self.inner.lock().expect("snapshot slot mutex poisoned") = Some(snapshot);
    }
}

/// Errors returned when spawning the actor.
#[derive(Debug)]
pub enum SpawnError {
    UnsupportedPlatform,
    NoRawFd,
    Io(io::Error),
    Pty(String),
    VtCore(VtCoreError),
    Init(String),
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => f.write_str("VT actor is only supported on Unix"),
            Self::NoRawFd => f.write_str("PTY master did not expose a Unix raw fd"),
            Self::Io(err) => write!(f, "IO error: {err}"),
            Self::Pty(err) => write!(f, "PTY error: {err}"),
            Self::VtCore(err) => write!(f, "VT core error: {err}"),
            Self::Init(err) => write!(f, "actor initialization failed: {err}"),
        }
    }
}

impl std::error::Error for SpawnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::VtCore(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for SpawnError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// Errors returned by command methods after the actor has been spawned.
#[derive(Debug)]
pub enum VtSessionError {
    Closed,
    Notify(io::Error),
}

impl fmt::Display for VtSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str("VT session actor is closed"),
            Self::Notify(err) => write!(f, "failed to notify VT actor: {err}"),
        }
    }
}

impl std::error::Error for VtSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Notify(err) => Some(err),
            _ => None,
        }
    }
}

/// Cross-thread queue for OSC 52 clipboard requests parsed by the actor. The
/// actor pushes, the UI thread drains. The shared mutex is fine here because
/// pushes happen at byte-feed rate (rare) and drains happen on the UI thread
/// at event-loop rate.
#[derive(Clone, Debug, Default)]
pub(crate) struct ClipboardQueue {
    inner: Arc<Mutex<VecDeque<ClipboardRequest>>>,
}

impl ClipboardQueue {
    fn new() -> Self {
        Self::default()
    }

    fn push_many(&self, requests: impl IntoIterator<Item = ClipboardRequest>) -> bool {
        let mut guard = self.inner.lock().expect("clipboard queue poisoned");
        let before = guard.len();
        guard.extend(requests);
        guard.len() > before
    }

    fn drain(&self) -> Vec<ClipboardRequest> {
        let mut guard = self.inner.lock().expect("clipboard queue poisoned");
        guard.drain(..).collect()
    }
}

/// Handle used by the UI thread to command a VT actor and read snapshots.
pub struct VtSessionHandle {
    commands: mpsc::Sender<VtCommand>,
    poller: Arc<polling::Poller>,
    slot: SnapshotSlot,
    content_dirty_pending: Arc<AtomicBool>,
    clipboard: ClipboardQueue,
    join: Option<JoinHandle<()>>,
}

impl VtSessionHandle {
    pub fn latest_snapshot(&self) -> Option<Arc<VtSnapshot>> {
        self.slot.latest_snapshot()
    }

    pub fn clear_content_dirty_pending(&self) {
        self.content_dirty_pending.store(false, Ordering::SeqCst);
    }

    /// Drain and return any OSC 52 clipboard requests parsed by the actor.
    /// Returns an empty vec when nothing is pending — callers wake on
    /// [`VtEvent::ClipboardActivity`] but should treat that signal as a
    /// hint and tolerate spurious drains.
    pub fn drain_clipboard_requests(&self) -> Vec<ClipboardRequest> {
        self.clipboard.drain()
    }

    pub fn write(&self, bytes: Bytes) -> Result<(), VtSessionError> {
        self.send(VtCommand::Write(bytes))
    }

    pub fn resize(&self, resize: Resize) -> Result<(), VtSessionError> {
        self.send(VtCommand::Resize {
            cols: resize.cols,
            rows: resize.rows,
            pixel_width: resize.pixel_width,
            pixel_height: resize.pixel_height,
        })
    }

    pub fn set_theme_colors(&self, colors: ThemeColors) -> Result<(), VtSessionError> {
        self.send(VtCommand::SetThemeColors(colors))
    }

    pub fn scroll_lines(&self, delta: i32) -> Result<(), VtSessionError> {
        self.send(VtCommand::ScrollLines(delta))
    }

    pub fn set_cursor_shape(&self, shape: CursorShape) -> Result<(), VtSessionError> {
        self.send(VtCommand::SetCursorShape(shape))
    }

    pub fn ack_rendered(&self, generation: u64) -> Result<(), VtSessionError> {
        self.send(VtCommand::AckRendered(generation))
    }

    /// Send shutdown, notify the actor, and wait for the IO thread to exit.
    pub fn join(mut self) -> thread::Result<()> {
        let _ = self.shutdown();
        if let Some(join) = self.join.take() {
            join.join()
        } else {
            Ok(())
        }
    }

    fn send(&self, command: VtCommand) -> Result<(), VtSessionError> {
        self.commands
            .send(command)
            .map_err(|_| VtSessionError::Closed)?;
        self.poller.notify().map_err(VtSessionError::Notify)
    }

    fn shutdown(&self) -> Result<(), VtSessionError> {
        self.commands
            .send(VtCommand::Shutdown)
            .map_err(|_| VtSessionError::Closed)?;
        self.poller.notify().map_err(VtSessionError::Notify)
    }
}

impl Drop for VtSessionHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(VtCommand::Shutdown);
        let _ = self.poller.notify();
        // Intentionally do not join: dropping the JoinHandle detaches the actor.
        let _ = self.join.take();
    }
}

/// Spawn a winit-free VT actor.
pub fn spawn_vt_session<F>(
    options: VtSessionOptions,
    event_sink: F,
) -> Result<VtSessionHandle, SpawnError>
where
    F: Fn(VtEvent) + Send + 'static,
{
    #[cfg(not(unix))]
    {
        let _ = (options, event_sink);
        Err(SpawnError::UnsupportedPlatform)
    }

    #[cfg(unix)]
    {
        spawn_vt_session_unix(options, event_sink)
    }
}

#[derive(Debug, Default)]
struct CoalescedCommands {
    writes: VecDeque<Bytes>,
    resize: Option<Resize>,
    theme: Option<ThemeColors>,
    scroll_delta: i32,
    cursor_shape: Option<CursorShape>,
    ack_generation: Option<u64>,
    shutdown: bool,
}

impl CoalescedCommands {
    fn drain(rx: &mpsc::Receiver<VtCommand>) -> Self {
        let mut out = Self::default();
        while let Ok(command) = rx.try_recv() {
            out.push(command);
            if out.shutdown {
                break;
            }
        }
        out
    }

    fn push(&mut self, command: VtCommand) {
        match command {
            VtCommand::Write(bytes) => self.writes.push_back(bytes),
            VtCommand::Resize {
                cols,
                rows,
                pixel_width,
                pixel_height,
            } => {
                self.resize = Some(Resize {
                    cols,
                    rows,
                    pixel_width,
                    pixel_height,
                });
            }
            VtCommand::SetThemeColors(colors) => self.theme = Some(colors),
            VtCommand::ScrollLines(delta) => {
                self.scroll_delta = self.scroll_delta.saturating_add(delta);
            }
            VtCommand::SetCursorShape(shape) => self.cursor_shape = Some(shape),
            VtCommand::AckRendered(generation) => {
                self.ack_generation = Some(
                    self.ack_generation
                        .map_or(generation, |current| current.max(generation)),
                );
            }
            VtCommand::Shutdown => self.shutdown = true,
        }
    }

    fn is_empty(&self) -> bool {
        self.writes.is_empty()
            && self.resize.is_none()
            && self.theme.is_none()
            && self.scroll_delta == 0
            && self.cursor_shape.is_none()
            && self.ack_generation.is_none()
            && !self.shutdown
    }
}

#[derive(Debug, Default)]
struct PendingWrites {
    queue: VecDeque<Bytes>,
}

impl PendingWrites {
    fn push(&mut self, bytes: Bytes) {
        if !bytes.is_empty() {
            self.queue.push_back(bytes);
        }
    }

    fn extend(&mut self, writes: VecDeque<Bytes>) {
        for bytes in writes {
            self.push(bytes);
        }
    }

    fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    fn flush(&mut self, writer: &mut dyn io::Write) -> io::Result<()> {
        while let Some(front) = self.queue.front_mut() {
            if !front.has_remaining() {
                self.queue.pop_front();
                continue;
            }

            match writer.write(front.chunk()) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "PTY writer returned zero bytes",
                    ));
                }
                Ok(n) => {
                    front.advance(n);
                    if !front.has_remaining() {
                        self.queue.pop_front();
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn queued_bytes(&self) -> Vec<u8> {
        self.queue
            .iter()
            .flat_map(|bytes| bytes.iter().copied())
            .collect()
    }
}

#[derive(Debug, Clone)]
struct SyncOutputGate {
    deadline: Option<Instant>,
    timeout: Duration,
}

impl Default for SyncOutputGate {
    fn default() -> Self {
        Self {
            deadline: None,
            timeout: SYNC_OUTPUT_TIMEOUT,
        }
    }
}

impl SyncOutputGate {
    fn after_parse_batch(&mut self, sync_active: bool, now: Instant) -> bool {
        if sync_active {
            match self.deadline {
                Some(deadline) if now >= deadline => {
                    self.deadline = None;
                    true
                }
                Some(_) => false,
                None => {
                    self.deadline = now.checked_add(self.timeout);
                    false
                }
            }
        } else {
            self.deadline = None;
            true
        }
    }

    fn timeout_from(&self, now: Instant) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(now))
    }

    fn on_watchdog(&mut self, sync_active: bool, now: Instant) -> bool {
        if !sync_active {
            self.deadline = None;
            return false;
        }

        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.deadline = None;
            true
        } else {
            false
        }
    }

    fn clear(&mut self) {
        self.deadline = None;
    }
}

struct ContentNotifier<F> {
    slot: SnapshotSlot,
    content_dirty_pending: Arc<AtomicBool>,
    clipboard: ClipboardQueue,
    event_sink: F,
}

impl<F> ContentNotifier<F>
where
    F: Fn(VtEvent),
{
    fn publish(&self, snapshot: Arc<VtSnapshot>) {
        self.slot.publish(snapshot);
        if !self.content_dirty_pending.swap(true, Ordering::SeqCst) {
            (self.event_sink)(VtEvent::ContentDirty);
        }
    }

    fn forward_clipboard(&self, requests: Vec<ClipboardRequest>) {
        if requests.is_empty() {
            return;
        }
        if self.clipboard.push_many(requests) {
            (self.event_sink)(VtEvent::ClipboardActivity);
        }
    }

    fn exited(&self) {
        (self.event_sink)(VtEvent::Exited);
    }
}

#[cfg(unix)]
mod unix_actor {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::{BorrowedFd, RawFd};

    use polling::{Event, Events, Poller};
    use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

    use crate::core::{VtCore, VtCoreOptions};

    pub(super) fn spawn_vt_session_unix<F>(
        options: VtSessionOptions,
        event_sink: F,
    ) -> Result<VtSessionHandle, SpawnError>
    where
        F: Fn(VtEvent) + Send + 'static,
    {
        let poller = Arc::new(Poller::new()?);
        let (command_tx, command_rx) = mpsc::channel();
        let slot = SnapshotSlot::new();
        let content_dirty_pending = Arc::new(AtomicBool::new(false));
        let clipboard = ClipboardQueue::new();
        let (init_tx, init_rx) = mpsc::sync_channel(1);

        let thread_poller = Arc::clone(&poller);
        let thread_slot = slot.clone();
        let thread_pending = Arc::clone(&content_dirty_pending);
        let thread_clipboard = clipboard.clone();

        let join = thread::Builder::new()
            .name("seance-vt-actor".into())
            .spawn(move || {
                let notifier = ContentNotifier {
                    slot: thread_slot,
                    content_dirty_pending: thread_pending,
                    clipboard: thread_clipboard,
                    event_sink,
                };
                match VtActor::new(options, command_rx, thread_poller, notifier) {
                    Ok(mut actor) => {
                        let _ = init_tx.send(Ok(()));
                        actor.run();
                    }
                    Err(err) => {
                        let _ = init_tx.send(Err(err));
                    }
                }
            })?;

        match init_rx.recv() {
            Ok(Ok(())) => Ok(VtSessionHandle {
                commands: command_tx,
                poller,
                slot,
                content_dirty_pending,
                clipboard,
                join: Some(join),
            }),
            Ok(Err(err)) => {
                let _ = join.join();
                Err(err)
            }
            Err(err) => {
                let _ = join.join();
                Err(SpawnError::Init(err.to_string()))
            }
        }
    }

    trait PtyAdapter: Read + Write + Send {
        fn resize(&mut self, size: PtySize);
        fn child_exited(&mut self) -> bool;
        fn kill_child(&mut self);
        fn raw_fd(&self) -> Option<RawFd> {
            None
        }
        /// Block briefly waiting for the PTY to become readable again. Used to
        /// coalesce close-following writes (see `PTY_READ_SETTLE`).
        fn wait_for_more(&mut self, _timeout: Duration) -> bool {
            false
        }
    }

    struct PortablePtyAdapter {
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        master: Box<dyn MasterPty + Send>,
        child: Box<dyn Child + Send + Sync>,
        fd: RawFd,
    }

    impl PortablePtyAdapter {
        fn open(options: VtSessionOptions) -> Result<Self, SpawnError> {
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: options.rows,
                    cols: options.cols,
                    pixel_width: options.pixel_width,
                    pixel_height: options.pixel_height,
                })
                .map_err(|err| SpawnError::Pty(err.to_string()))?;
            // GUI launches (e.g. macOS `.app` bundles) inherit no TERM from
            // launchd, which leaves terminfo-based programs (htop via ncurses,
            // tmux via libtinfo) unable to start. Force a widely-installed
            // terminfo entry so the child shell can describe us. Revisit if we
            // ship our own terminfo.
            let mut command = CommandBuilder::new_default_prog();
            command.env("TERM", "xterm-256color");
            command.env("COLORTERM", "truecolor");
            command.env("TERM_PROGRAM", "seance");
            command.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
            let child = pair
                .slave
                .spawn_command(command)
                .map_err(|err| SpawnError::Pty(err.to_string()))?;
            let reader = pair
                .master
                .try_clone_reader()
                .map_err(|err| SpawnError::Pty(err.to_string()))?;
            let writer = pair
                .master
                .take_writer()
                .map_err(|err| SpawnError::Pty(err.to_string()))?;
            let fd = pair.master.as_raw_fd().ok_or(SpawnError::NoRawFd)?;
            set_nonblocking(fd)?;
            Ok(Self {
                reader,
                writer,
                master: pair.master,
                child,
                fd,
            })
        }
    }

    impl Read for PortablePtyAdapter {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.reader.read(buf)
        }
    }

    impl Write for PortablePtyAdapter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writer.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.writer.flush()
        }
    }

    impl PtyAdapter for PortablePtyAdapter {
        fn resize(&mut self, size: PtySize) {
            let _ = self.master.resize(size);
        }

        fn child_exited(&mut self) -> bool {
            !matches!(self.child.try_wait(), Ok(None))
        }

        fn kill_child(&mut self) {
            let _ = self.child.kill();
        }

        fn raw_fd(&self) -> Option<RawFd> {
            Some(self.fd)
        }

        fn wait_for_more(&mut self, timeout: Duration) -> bool {
            let mut pollfd = libc::pollfd {
                fd: self.fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let timeout_ms = timeout.as_millis().clamp(1, i32::MAX as u128) as i32;
            // SAFETY: pollfd is a single owned slot we initialized above; the
            // PTY fd remains valid for the adapter's lifetime.
            let r = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
            r > 0 && (pollfd.revents & libc::POLLIN) != 0
        }
    }

    struct VtActor<F, P>
    where
        F: Fn(VtEvent),
        P: PtyAdapter,
    {
        core: VtCore,
        pty: P,
        commands: mpsc::Receiver<VtCommand>,
        pending_writes: PendingWrites,
        notifier: ContentNotifier<F>,
        poller: Arc<Poller>,
        fd: Option<RawFd>,
        sync_gate: SyncOutputGate,
        coalesce_delay: Duration,
        last_input_at: Option<Instant>,
    }

    impl<F> VtActor<F, PortablePtyAdapter>
    where
        F: Fn(VtEvent),
    {
        fn new(
            options: VtSessionOptions,
            commands: mpsc::Receiver<VtCommand>,
            poller: Arc<Poller>,
            notifier: ContentNotifier<F>,
        ) -> Result<Self, SpawnError> {
            let pty = PortablePtyAdapter::open(options.clone())?;
            Self::with_pty(options, pty, commands, poller, notifier)
        }
    }

    impl<F, P> VtActor<F, P>
    where
        F: Fn(VtEvent),
        P: PtyAdapter,
    {
        fn with_pty(
            options: VtSessionOptions,
            pty: P,
            commands: mpsc::Receiver<VtCommand>,
            poller: Arc<Poller>,
            notifier: ContentNotifier<F>,
        ) -> Result<Self, SpawnError> {
            let core = VtCore::new(VtCoreOptions {
                cols: options.cols,
                rows: options.rows,
                pixel_width: options.pixel_width,
                pixel_height: options.pixel_height,
                max_scrollback: options.max_scrollback,
                initial_cursor_shape: options.initial_cursor_shape,
            })
            .map_err(SpawnError::VtCore)?;

            let fd = pty.raw_fd();
            let mut actor = Self {
                core,
                pty,
                commands,
                pending_writes: PendingWrites::default(),
                notifier,
                poller,
                fd,
                sync_gate: SyncOutputGate::default(),
                coalesce_delay: Duration::from_millis(u64::from(options.coalesce_delay_ms)),
                last_input_at: None,
            };
            actor.drain_responses();
            actor.publish_snapshot().map_err(SpawnError::VtCore)?;

            if let Some(fd) = actor.fd {
                let mut interest = Event::readable(PTY_KEY);
                interest.set_interrupt(true);
                unsafe {
                    actor.poller.add(fd, interest)?;
                }
            }

            Ok(actor)
        }

        fn run(&mut self) {
            let mut events = Events::new();
            loop {
                let _span = tracing::trace_span!("vt::tick").entered();
                if self.drain_and_apply_commands() {
                    break;
                }
                if self.flush_pending_writes().is_err() {
                    self.notifier.exited();
                    break;
                }
                if self.child_exited() {
                    self.notifier.exited();
                    break;
                }

                if let Err(err) = self.reregister() {
                    tracing::warn!("VT actor failed to register PTY interest: {err}");
                    self.notifier.exited();
                    break;
                }

                events.clear();
                let timeout = self.sync_gate.timeout_from(Instant::now());
                match self.poller.wait(&mut events, timeout) {
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!("VT actor poll failed: {err}");
                        self.notifier.exited();
                        break;
                    }
                }

                if self.drain_and_apply_commands() {
                    break;
                }

                let mut readable = false;
                let mut writable = false;
                let mut interrupted = false;
                for event in events.iter() {
                    if event.key == PTY_KEY {
                        readable |= event.readable;
                        writable |= event.writable;
                        interrupted |= event.is_interrupt();
                    }
                }

                if readable || interrupted {
                    match self.read_pty_batch() {
                        Ok(ReadOutcome::Alive) => {}
                        Ok(ReadOutcome::Eof) => {
                            self.notifier.exited();
                            break;
                        }
                        Err(err) => {
                            tracing::warn!("VT actor PTY read failed: {err}");
                            self.notifier.exited();
                            break;
                        }
                    }
                }
                if writable && self.flush_pending_writes().is_err() {
                    self.notifier.exited();
                    break;
                }

                if events.is_empty()
                    && self
                        .sync_gate
                        .on_watchdog(self.sync_active(), Instant::now())
                {
                    let _ = self.publish_snapshot_if_dirty();
                }
            }

            tracing::info!("vt actor exiting");
            if let Some(fd) = self.fd {
                let _ = self.poller.delete(unsafe { BorrowedFd::borrow_raw(fd) });
            }
            if !self.child_exited() {
                self.pty.kill_child();
            }
        }

        fn reregister(&self) -> io::Result<()> {
            let Some(fd) = self.fd else {
                return Ok(());
            };
            let mut interest = Event::new(PTY_KEY, true, !self.pending_writes.is_empty());
            interest.set_interrupt(true);
            self.poller
                .modify(unsafe { BorrowedFd::borrow_raw(fd) }, interest)
        }

        fn drain_and_apply_commands(&mut self) -> bool {
            let batch = CoalescedCommands::drain(&self.commands);
            if batch.is_empty() {
                return false;
            }
            if batch.shutdown {
                return true;
            }

            if let Some(generation) = batch.ack_generation {
                self.core.ack_rendered(generation);
            }
            if let Some(resize) = batch.resize {
                self.apply_resize(resize);
                self.sync_gate.clear();
                let _ = self.publish_snapshot();
            }
            if let Some(theme) = batch.theme {
                self.core
                    .set_theme_colors(theme.fg, theme.bg, theme.cursor, theme.palette);
                let _ = self.publish_snapshot();
            }
            if let Some(shape) = batch.cursor_shape {
                self.core.seed_cursor_shape(shape);
                self.drain_responses();
                let _ = self.publish_snapshot();
            }
            if batch.scroll_delta != 0 {
                self.core.scroll_lines(batch.scroll_delta);
                let _ = self.publish_snapshot();
            }
            if !batch.writes.is_empty() {
                self.last_input_at = Some(Instant::now());
            }
            self.pending_writes.extend(batch.writes);
            false
        }

        fn apply_resize(&mut self, resize: Resize) {
            if let Err(err) = self.core.resize(
                resize.cols,
                resize.rows,
                resize.pixel_width,
                resize.pixel_height,
            ) {
                tracing::warn!("VT actor failed to resize VT core: {err}");
            }
            self.pty.resize(PtySize {
                rows: resize.rows,
                cols: resize.cols,
                pixel_width: resize.pixel_width,
                pixel_height: resize.pixel_height,
            });
        }

        fn flush_pending_writes(&mut self) -> io::Result<()> {
            self.pending_writes.flush(&mut self.pty)
        }

        fn read_pty_batch(&mut self) -> io::Result<ReadOutcome> {
            let _span = tracing::trace_span!("vt::read_pty").entered();
            let mut total = 0usize;
            let mut changed = false;
            let mut buf = [0u8; READ_CHUNK];
            let mut settled = false;

            while total < MAX_READ_PER_TICK {
                match self.pty.read(&mut buf) {
                    Ok(0) => return Ok(ReadOutcome::Eof),
                    Ok(n) => {
                        total += n;
                        changed = true;
                        self.core.feed(&buf[..n]);
                        self.drain_responses();
                    }
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        let window = settle_window(self.coalesce_delay, self.recent_input());
                        if !changed
                            || settled
                            || window.is_zero()
                            || !self.pty.wait_for_more(window)
                        {
                            break;
                        }
                        settled = true;
                    }
                    Err(err) => return Err(err),
                }
            }

            if changed
                && self
                    .sync_gate
                    .after_parse_batch(self.sync_active(), Instant::now())
            {
                let _ = self.publish_snapshot_if_dirty();
            }
            Ok(ReadOutcome::Alive)
        }

        fn drain_responses(&mut self) {
            for bytes in self.core.drain_responses() {
                self.pending_writes.push(bytes);
            }
            self.notifier
                .forward_clipboard(self.core.drain_clipboard_requests());
        }

        /// Publish a snapshot only if the parse produced visible content
        /// changes. Mode toggles like `\x1b[?2004h/l`, `\x1b[?2031h/l`, and
        /// `\x1b[=Nu` round-trip through libghostty without dirtying any
        /// rows; under rapid history navigation the shell emits a pair of
        /// these around every prompt redraw, and forwarding them as
        /// `PaneUpdate`s drives a full GPU redraw per keystroke for zero
        /// painted-pixel difference. The actor keeps the generation
        /// increment so subsequent real changes still ship with the
        /// correct generation; only the slot push, the wake, and the
        /// downstream redraw are skipped for the no-op snapshot.
        fn publish_snapshot_if_dirty(&mut self) -> Result<(), VtCoreError> {
            let snapshot = self.core.snapshot()?;
            if matches!(snapshot.dirty, crate::DirtySnapshot::Clean) {
                return Ok(());
            }
            self.notifier.publish(Arc::new(snapshot));
            Ok(())
        }

        fn publish_snapshot(&mut self) -> Result<(), VtCoreError> {
            let snapshot = self.core.snapshot()?;
            self.notifier.publish(Arc::new(snapshot));
            Ok(())
        }

        fn sync_active(&self) -> bool {
            self.core.sync_active()
        }

        fn recent_input(&self) -> bool {
            self.last_input_at
                .is_some_and(|at| at.elapsed() < INPUT_ACTIVITY_WINDOW)
        }

        fn child_exited(&mut self) -> bool {
            self.pty.child_exited()
        }
    }

    enum ReadOutcome {
        Alive,
        Eof,
    }

    fn set_nonblocking(fd: RawFd) -> io::Result<()> {
        // SAFETY: `fd` is a valid PTY master file descriptor owned by the actor.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` remains valid and we only add O_NONBLOCK to its flags.
        let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::DirtySnapshot;

        type TestSink = Box<dyn Fn(VtEvent)>;
        type ActorFixture = (
            mpsc::Sender<VtCommand>,
            VtActor<TestSink, ScriptedPtyAdapter>,
            SnapshotSlot,
            Arc<AtomicBool>,
            Arc<Mutex<Vec<VtEvent>>>,
            ClipboardQueue,
        );

        enum ScriptRead {
            Data(Bytes),
            WouldBlock,
            Eof,
        }

        #[derive(Default)]
        struct ScriptedPtyAdapter {
            reads: VecDeque<ScriptRead>,
            written: Vec<u8>,
            resizes: Vec<PtySize>,
            exited: bool,
            killed: bool,
        }

        impl ScriptedPtyAdapter {
            fn new(reads: impl IntoIterator<Item = ScriptRead>) -> Self {
                Self {
                    reads: reads.into_iter().collect(),
                    ..Self::default()
                }
            }

            fn push_read(&mut self, bytes: &'static [u8]) {
                self.reads
                    .push_back(ScriptRead::Data(Bytes::from_static(bytes)));
            }
        }

        impl Read for ScriptedPtyAdapter {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                match self.reads.pop_front() {
                    Some(ScriptRead::Data(bytes)) => {
                        let n = bytes.len().min(buf.len());
                        buf[..n].copy_from_slice(&bytes[..n]);
                        if n < bytes.len() {
                            self.reads.push_front(ScriptRead::Data(bytes.slice(n..)));
                        }
                        Ok(n)
                    }
                    Some(ScriptRead::WouldBlock) => Err(io::Error::from(io::ErrorKind::WouldBlock)),
                    Some(ScriptRead::Eof) => Ok(0),
                    None => Err(io::Error::from(io::ErrorKind::WouldBlock)),
                }
            }
        }

        impl Write for ScriptedPtyAdapter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.written.extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        impl PtyAdapter for ScriptedPtyAdapter {
            fn resize(&mut self, size: PtySize) {
                self.resizes.push(size);
            }

            fn child_exited(&mut self) -> bool {
                self.exited
            }

            fn kill_child(&mut self) {
                self.killed = true;
            }

            fn wait_for_more(&mut self, _timeout: Duration) -> bool {
                matches!(
                    self.reads.front(),
                    Some(ScriptRead::Data(_)) | Some(ScriptRead::Eof)
                )
            }
        }

        fn actor_with_script(reads: impl IntoIterator<Item = ScriptRead>) -> ActorFixture {
            let poller = Arc::new(Poller::new().unwrap());
            let (tx, rx) = mpsc::channel();
            let slot = SnapshotSlot::new();
            let pending = Arc::new(AtomicBool::new(false));
            let clipboard = ClipboardQueue::new();
            let events = Arc::new(Mutex::new(Vec::new()));
            let sink_events = Arc::clone(&events);
            let notifier = ContentNotifier {
                slot: slot.clone(),
                content_dirty_pending: Arc::clone(&pending),
                clipboard: clipboard.clone(),
                event_sink: Box::new(move |event| sink_events.lock().unwrap().push(event))
                    as TestSink,
            };
            let actor = VtActor::with_pty(
                VtSessionOptions {
                    cols: 8,
                    rows: 3,
                    pixel_width: 80,
                    pixel_height: 30,
                    initial_cursor_shape: CursorShape::Block,
                    ..VtSessionOptions::default()
                },
                ScriptedPtyAdapter::new(reads),
                rx,
                poller,
                notifier,
            )
            .expect("scripted actor should construct");
            (tx, actor, slot, pending, events, clipboard)
        }

        fn clear_wake_state(pending: &AtomicBool, events: &Mutex<Vec<VtEvent>>) {
            pending.store(false, Ordering::SeqCst);
            events.lock().unwrap().clear();
        }

        #[test]
        fn settle_window_widens_for_bursts_and_floors_for_input() {
            let delay = Duration::from_millis(8);
            // No recent input: the full configured window coalesces bursts.
            assert_eq!(settle_window(delay, false), delay);
            // Recent keystroke: capped to the flash floor so echo is prompt.
            assert_eq!(settle_window(delay, true), PTY_READ_SETTLE);
            // A sub-floor delay is never widened by the input cap.
            let tiny = Duration::from_micros(500);
            assert_eq!(settle_window(tiny, true), tiny);
            // Zero disables the settle regardless of input state.
            assert!(settle_window(Duration::ZERO, false).is_zero());
            assert!(settle_window(Duration::ZERO, true).is_zero());
        }

        #[test]
        fn read_pty_batch_skips_settle_when_coalesce_disabled() {
            let (_tx, mut actor, slot, pending, events, _clip) = actor_with_script([
                ScriptRead::Data(Bytes::from_static(b"\x1b[H\x1b[2J")),
                ScriptRead::WouldBlock,
                ScriptRead::Data(Bytes::from_static(b"$ ")),
            ]);
            actor.coalesce_delay = Duration::ZERO;
            clear_wake_state(&pending, &events);

            assert!(matches!(actor.read_pty_batch(), Ok(ReadOutcome::Alive)));

            // With the settle disabled the second write stays queued for the
            // next batch rather than coalescing into this snapshot.
            let snapshot = slot.latest_snapshot().unwrap();
            assert_ne!(snapshot.cell_text(&snapshot.cells[0]), "$");
            assert!(matches!(actor.pty.reads.front(), Some(ScriptRead::Data(_))));
        }

        #[test]
        fn scripted_actor_publishes_initial_snapshot() {
            let (_tx, _actor, slot, _pending, events, _clip) = actor_with_script([]);
            assert!(slot.latest_snapshot().is_some());
            assert_eq!(events.lock().unwrap().as_slice(), &[VtEvent::ContentDirty]);
        }

        #[test]
        fn scripted_actor_preserves_write_order() {
            let (tx, mut actor, _slot, _pending, _events, _clip) = actor_with_script([]);
            tx.send(VtCommand::Write(Bytes::from_static(b"a"))).unwrap();
            tx.send(VtCommand::Write(Bytes::from_static(b"bc")))
                .unwrap();

            assert!(!actor.drain_and_apply_commands());
            actor.flush_pending_writes().unwrap();

            assert_eq!(actor.pty.written, b"abc");
        }

        #[test]
        fn scripted_actor_reads_pty_bytes_into_vt_core() {
            let (_tx, mut actor, slot, pending, events, _clip) = actor_with_script([]);
            clear_wake_state(&pending, &events);
            actor.pty.push_read(b"hi");

            assert!(matches!(actor.read_pty_batch(), Ok(ReadOutcome::Alive)));

            let snapshot = slot.latest_snapshot().unwrap();
            assert_eq!(snapshot.cell_text(&snapshot.cells[0]), "h");
            assert_eq!(snapshot.cell_text(&snapshot.cells[1]), "i");
            assert_eq!(events.lock().unwrap().as_slice(), &[VtEvent::ContentDirty]);
        }

        #[test]
        fn scripted_actor_applies_visible_commands_without_reordering() {
            let (tx, mut actor, slot, pending, events, _clip) = actor_with_script([]);
            clear_wake_state(&pending, &events);
            tx.send(VtCommand::Resize {
                cols: 10,
                rows: 4,
                pixel_width: 100,
                pixel_height: 40,
            })
            .unwrap();
            tx.send(VtCommand::SetThemeColors(ThemeColors {
                fg: [1, 2, 3],
                bg: [4, 5, 6],
                cursor: [7, 8, 9],
                palette: [[0, 0, 0]; 256],
            }))
            .unwrap();
            tx.send(VtCommand::SetCursorShape(CursorShape::Underline))
                .unwrap();
            tx.send(VtCommand::ScrollLines(1)).unwrap();

            assert!(!actor.drain_and_apply_commands());

            assert_eq!(actor.pty.resizes.len(), 1);
            assert_eq!(actor.pty.resizes[0].cols, 10);
            assert_eq!(actor.pty.resizes[0].rows, 4);
            assert_eq!(slot.latest_snapshot().unwrap().cols, 10);
            assert_eq!(events.lock().unwrap().as_slice(), &[VtEvent::ContentDirty]);
        }

        #[test]
        fn scripted_actor_ack_does_not_publish() {
            let (tx, mut actor, slot, pending, events, _clip) = actor_with_script([]);
            let generation = slot.latest_snapshot().unwrap().generation;
            clear_wake_state(&pending, &events);
            tx.send(VtCommand::AckRendered(generation)).unwrap();

            assert!(!actor.drain_and_apply_commands());

            assert!(events.lock().unwrap().is_empty());
            assert_eq!(slot.latest_snapshot().unwrap().generation, generation);
        }

        #[test]
        fn dirty_rows_survive_actor_publication_until_render_ack() {
            let (tx, mut actor, slot, pending, events, _clip) = actor_with_script([]);
            let initial = slot.latest_snapshot().unwrap();
            tx.send(VtCommand::AckRendered(initial.generation)).unwrap();
            assert!(!actor.drain_and_apply_commands());
            clear_wake_state(&pending, &events);

            actor.pty.push_read(b"x");
            assert!(matches!(actor.read_pty_batch(), Ok(ReadOutcome::Alive)));
            let dirty = slot.latest_snapshot().unwrap();
            assert_eq!(dirty.dirty, DirtySnapshot::Partial(vec![0]));

            clear_wake_state(&pending, &events);
            actor.publish_snapshot().unwrap();
            let republished = slot.latest_snapshot().unwrap();
            assert_eq!(republished.dirty, DirtySnapshot::Partial(vec![0]));

            tx.send(VtCommand::AckRendered(republished.generation))
                .unwrap();
            assert!(!actor.drain_and_apply_commands());
            actor.publish_snapshot().unwrap();
            assert_eq!(slot.latest_snapshot().unwrap().dirty, DirtySnapshot::Clean);
        }

        #[test]
        fn scripted_actor_reports_eof_deterministically() {
            let (_tx, mut actor, _slot, _pending, _events, _clip) =
                actor_with_script([ScriptRead::Eof]);
            assert!(matches!(actor.read_pty_batch(), Ok(ReadOutcome::Eof)));
        }

        #[test]
        fn read_pty_batch_coalesces_split_writes_via_settle() {
            // A WouldBlock between two data chunks models the kernel signaling
            // readability after the shell's first write but before its second
            // — the exact race that paints the cleared-with-home-cursor frame
            // on Ctrl-L in release builds.
            let (_tx, mut actor, slot, pending, events, _clip) = actor_with_script([
                ScriptRead::Data(Bytes::from_static(b"\x1b[H\x1b[2J")),
                ScriptRead::WouldBlock,
                ScriptRead::Data(Bytes::from_static(b"$ ")),
            ]);
            clear_wake_state(&pending, &events);

            assert!(matches!(actor.read_pty_batch(), Ok(ReadOutcome::Alive)));

            assert_eq!(events.lock().unwrap().as_slice(), &[VtEvent::ContentDirty]);
            let snapshot = slot.latest_snapshot().unwrap();
            assert_eq!(snapshot.cell_text(&snapshot.cells[0]), "$");
        }

        #[test]
        fn read_pty_batch_settles_at_most_once_per_batch() {
            let (_tx, mut actor, slot, pending, events, _clip) = actor_with_script([
                ScriptRead::Data(Bytes::from_static(b"a")),
                ScriptRead::WouldBlock,
                ScriptRead::Data(Bytes::from_static(b"b")),
                ScriptRead::WouldBlock,
                ScriptRead::Data(Bytes::from_static(b"c")),
            ]);
            clear_wake_state(&pending, &events);

            assert!(matches!(actor.read_pty_batch(), Ok(ReadOutcome::Alive)));

            let snapshot = slot.latest_snapshot().unwrap();
            assert_eq!(snapshot.cell_text(&snapshot.cells[0]), "a");
            assert_eq!(snapshot.cell_text(&snapshot.cells[1]), "b");
            assert_ne!(snapshot.cell_text(&snapshot.cells[2]), "c");
            assert!(
                matches!(actor.pty.reads.front(), Some(ScriptRead::Data(_))),
                "third chunk must remain queued for the next batch",
            );
        }

        #[test]
        fn osc52_write_surfaces_clipboard_request_and_wakes() {
            let (_tx, mut actor, _slot, pending, events, clipboard) = actor_with_script([]);
            clear_wake_state(&pending, &events);
            // OSC 52 ; c ; aGVsbG8= BEL  → set clipboard to "hello"
            actor.pty.push_read(b"\x1b]52;c;aGVsbG8=\x07");

            assert!(matches!(actor.read_pty_batch(), Ok(ReadOutcome::Alive)));

            let requests = clipboard.drain();
            assert_eq!(requests, vec![ClipboardRequest::Write(b"hello".to_vec())],);
            assert!(events.lock().unwrap().contains(&VtEvent::ClipboardActivity),);
        }

        #[test]
        fn osc52_read_request_surfaces_distinct_variant() {
            let (_tx, mut actor, _slot, pending, events, clipboard) = actor_with_script([]);
            clear_wake_state(&pending, &events);
            actor.pty.push_read(b"\x1b]52;c;?\x07");

            assert!(matches!(actor.read_pty_batch(), Ok(ReadOutcome::Alive)));

            assert_eq!(clipboard.drain(), vec![ClipboardRequest::Read]);
        }
    }
}

#[cfg(unix)]
use unix_actor::spawn_vt_session_unix;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CellColor;

    fn test_snapshot(text: &str) -> Arc<VtSnapshot> {
        let mut snapshot = VtSnapshot::empty(1, 1);
        snapshot.push_cell(
            text,
            CellColor::Default,
            CellColor::Default,
            crate::CellAttrs::default(),
        );
        Arc::new(snapshot)
    }

    fn theme(seed: u8) -> ThemeColors {
        ThemeColors {
            fg: [seed, 1, 2],
            bg: [3, seed, 4],
            cursor: [5, 6, seed],
            palette: [[seed, seed, seed]; 256],
        }
    }

    #[test]
    fn snapshot_slot_returns_newest_snapshot() {
        let slot = SnapshotSlot::new();
        assert!(slot.latest_snapshot().is_none());

        let first = test_snapshot("a");
        let second = test_snapshot("b");
        slot.publish(Arc::clone(&first));
        assert!(Arc::ptr_eq(&slot.latest_snapshot().unwrap(), &first));
        slot.publish(Arc::clone(&second));
        assert!(Arc::ptr_eq(&slot.latest_snapshot().unwrap(), &second));
    }

    #[test]
    fn content_dirty_dedupe_allows_clear_before_clone_without_missed_wake() {
        let slot = SnapshotSlot::new();
        let pending = Arc::new(AtomicBool::new(false));
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        let notifier = ContentNotifier {
            slot: slot.clone(),
            content_dirty_pending: Arc::clone(&pending),
            clipboard: ClipboardQueue::new(),
            event_sink: move |event| sink_events.lock().unwrap().push(event),
        };

        notifier.publish(test_snapshot("a"));
        notifier.publish(test_snapshot("b"));
        assert_eq!(events.lock().unwrap().as_slice(), &[VtEvent::ContentDirty]);
        assert_eq!(
            slot.latest_snapshot()
                .unwrap()
                .cell_text(&slot.latest_snapshot().unwrap().cells[0]),
            "b"
        );

        pending.store(false, Ordering::SeqCst);
        notifier.publish(test_snapshot("c"));
        assert_eq!(
            events.lock().unwrap().as_slice(),
            &[VtEvent::ContentDirty, VtEvent::ContentDirty]
        );
        let latest = slot.latest_snapshot().unwrap();
        assert_eq!(latest.cell_text(&latest.cells[0]), "c");
    }

    #[test]
    fn command_coalescing_preserves_write_order_and_latest_state() {
        let (tx, rx) = mpsc::channel();
        tx.send(VtCommand::Write(Bytes::from_static(b"a"))).unwrap();
        tx.send(VtCommand::Resize {
            cols: 80,
            rows: 24,
            pixel_width: 800,
            pixel_height: 384,
        })
        .unwrap();
        tx.send(VtCommand::SetThemeColors(theme(1))).unwrap();
        tx.send(VtCommand::ScrollLines(2)).unwrap();
        tx.send(VtCommand::Write(Bytes::from_static(b"b"))).unwrap();
        tx.send(VtCommand::Resize {
            cols: 100,
            rows: 40,
            pixel_width: 1000,
            pixel_height: 800,
        })
        .unwrap();
        tx.send(VtCommand::SetThemeColors(theme(2))).unwrap();
        tx.send(VtCommand::ScrollLines(-5)).unwrap();
        tx.send(VtCommand::SetCursorShape(CursorShape::Bar))
            .unwrap();
        tx.send(VtCommand::SetCursorShape(CursorShape::Underline))
            .unwrap();
        tx.send(VtCommand::AckRendered(3)).unwrap();
        tx.send(VtCommand::AckRendered(7)).unwrap();
        tx.send(VtCommand::AckRendered(5)).unwrap();

        let batch = CoalescedCommands::drain(&rx);
        assert_eq!(
            batch.writes.into_iter().collect::<Vec<_>>(),
            vec![Bytes::from_static(b"a"), Bytes::from_static(b"b")]
        );
        assert_eq!(
            batch.resize,
            Some(Resize {
                cols: 100,
                rows: 40,
                pixel_width: 1000,
                pixel_height: 800,
            })
        );
        assert_eq!(batch.theme, Some(theme(2)));
        assert_eq!(batch.scroll_delta, -3);
        assert_eq!(batch.cursor_shape, Some(CursorShape::Underline));
        assert_eq!(batch.ack_generation, Some(7));
        assert!(!batch.shutdown);
    }

    struct ScriptedWriter {
        actions: VecDeque<io::Result<usize>>,
        written: Vec<u8>,
    }

    impl ScriptedWriter {
        fn new(actions: impl IntoIterator<Item = io::Result<usize>>) -> Self {
            Self {
                actions: actions.into_iter().collect(),
                written: Vec::new(),
            }
        }
    }

    impl io::Write for ScriptedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let action = self.actions.pop_front().unwrap_or(Ok(buf.len()));
            if let Ok(n) = action {
                self.written.extend_from_slice(&buf[..n]);
                Ok(n)
            } else {
                action
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn pending_writes_handle_partial_interrupt_wouldblock_and_drain() {
        let mut pending = PendingWrites::default();
        pending.push(Bytes::from_static(b"abcdef"));

        let mut writer = ScriptedWriter::new([
            Ok(2),
            Err(io::Error::from(io::ErrorKind::Interrupted)),
            Ok(3),
            Err(io::Error::from(io::ErrorKind::WouldBlock)),
        ]);
        pending.flush(&mut writer).unwrap();
        assert_eq!(writer.written, b"abcde");
        assert_eq!(pending.queued_bytes(), b"f");

        let mut writer = ScriptedWriter::new([Ok(1)]);
        pending.flush(&mut writer).unwrap();
        assert_eq!(writer.written, b"f");
        assert!(pending.is_empty());
    }

    #[test]
    fn sync_output_gate_suppresses_exits_and_times_out() {
        let now = Instant::now();
        let mut gate = SyncOutputGate::default();

        assert!(!gate.after_parse_batch(true, now));
        assert!(gate.deadline.is_some());
        assert_eq!(gate.timeout_from(now), Some(SYNC_OUTPUT_TIMEOUT));

        assert!(gate.after_parse_batch(false, now + Duration::from_millis(1)));
        assert!(gate.deadline.is_none());

        assert!(!gate.after_parse_batch(true, now));
        assert!(!gate.on_watchdog(true, now + Duration::from_millis(149)));
        assert!(gate.on_watchdog(true, now + Duration::from_millis(150)));
        assert!(gate.deadline.is_none());
    }

    #[test]
    fn dropping_handle_sends_shutdown_without_joining() {
        let poller = Arc::new(polling::Poller::new().unwrap());
        let (tx, rx) = mpsc::channel();
        let saw_shutdown = Arc::new(AtomicBool::new(false));
        let saw_shutdown_thread = Arc::clone(&saw_shutdown);
        let join = thread::spawn(move || {
            if matches!(rx.recv(), Ok(VtCommand::Shutdown)) {
                saw_shutdown_thread.store(true, Ordering::SeqCst);
            }
            thread::sleep(Duration::from_millis(250));
        });

        let handle = VtSessionHandle {
            commands: tx,
            poller,
            slot: SnapshotSlot::new(),
            content_dirty_pending: Arc::new(AtomicBool::new(false)),
            clipboard: ClipboardQueue::new(),
            join: Some(join),
        };

        let start = Instant::now();
        drop(handle);
        assert!(start.elapsed() < Duration::from_millis(100));

        let deadline = Instant::now() + Duration::from_millis(100);
        while !saw_shutdown.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(saw_shutdown.load(Ordering::SeqCst));
    }

    #[cfg(unix)]
    #[test]
    fn actor_spawn_publishes_initial_snapshot_and_join_works() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        let handle = spawn_vt_session(
            VtSessionOptions {
                cols: 8,
                rows: 4,
                pixel_width: 80,
                pixel_height: 40,
                initial_cursor_shape: CursorShape::Block,
                ..VtSessionOptions::default()
            },
            move |event| sink_events.lock().unwrap().push(event),
        )
        .expect("actor should spawn on Unix");

        assert!(handle.latest_snapshot().is_some());
        assert_eq!(
            events.lock().unwrap().first().copied(),
            Some(VtEvent::ContentDirty)
        );
        handle.join().expect("actor thread should join cleanly");
    }
}
