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

use crate::frame::CursorShape;
use crate::snapshot::VtSnapshot;

const SYNC_OUTPUT_TIMEOUT: Duration = Duration::from_millis(150);

#[cfg(unix)]
const READ_CHUNK: usize = 16 * 1024;
#[cfg(unix)]
const MAX_READ_PER_TICK: usize = 256 * 1024;
#[cfg(unix)]
const MAX_SCROLLBACK: usize = 10_000;
#[cfg(unix)]
const KITTY_IMAGE_STORAGE_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
#[cfg(unix)]
const PTY_KEY: usize = 0;

/// Options used when spawning a VT session actor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VtSessionOptions {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
    pub initial_cursor_shape: CursorShape,
}

impl Default for VtSessionOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            pixel_width: 800,
            pixel_height: 384,
            initial_cursor_shape: CursorShape::Block,
        }
    }
}

/// Pixel and cell dimensions for a resize command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resize {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl Resize {
    fn cell_pixels(self) -> (u32, u32) {
        cell_px(self.cols, self.rows, self.pixel_width, self.pixel_height)
    }
}

/// Default colors/palette to seed into libghostty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeColors {
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub cursor: [u8; 3],
    pub palette: [[u8; 3]; 256],
}

/// Public events emitted by the VT actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VtEvent {
    ContentDirty,
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
    Vt(&'static str),
    Init(String),
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => f.write_str("VT actor is only supported on Unix"),
            Self::NoRawFd => f.write_str("PTY master did not expose a Unix raw fd"),
            Self::Io(err) => write!(f, "IO error: {err}"),
            Self::Pty(err) => write!(f, "PTY error: {err}"),
            Self::Vt(op) => write!(f, "libghostty operation failed: {op}"),
            Self::Init(err) => write!(f, "actor initialization failed: {err}"),
        }
    }
}

impl std::error::Error for SpawnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
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

/// Handle used by the UI thread to command a VT actor and read snapshots.
pub struct VtSessionHandle {
    commands: mpsc::Sender<VtCommand>,
    poller: Arc<polling::Poller>,
    slot: SnapshotSlot,
    content_dirty_pending: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl VtSessionHandle {
    pub fn latest_snapshot(&self) -> Option<Arc<VtSnapshot>> {
        self.slot.latest_snapshot()
    }

    pub fn clear_content_dirty_pending(&self) {
        self.content_dirty_pending.store(false, Ordering::SeqCst);
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
            VtCommand::Shutdown => self.shutdown = true,
        }
    }

    fn is_empty(&self) -> bool {
        self.writes.is_empty()
            && self.resize.is_none()
            && self.theme.is_none()
            && self.scroll_delta == 0
            && self.cursor_shape.is_none()
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

    fn exited(&self) {
        (self.event_sink)(VtEvent::Exited);
    }
}

#[cfg(unix)]
mod unix_actor {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::{BorrowedFd, RawFd};

    use libghostty_vt::RenderState;
    use libghostty_vt::style::RgbColor;
    use libghostty_vt::terminal::{Mode, ScrollViewport};
    use libghostty_vt::{Terminal as VtTerminal, TerminalOptions};
    use polling::{Event, Events, Poller};
    use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

    use crate::frame_source::build_snapshot_from_parts;
    use crate::terminal::install_png_decoder_for_this_thread;

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
        let (init_tx, init_rx) = mpsc::sync_channel(1);

        let thread_poller = Arc::clone(&poller);
        let thread_slot = slot.clone();
        let thread_pending = Arc::clone(&content_dirty_pending);

        let join = thread::Builder::new()
            .name("seance-vt-actor".into())
            .spawn(move || {
                let notifier = ContentNotifier {
                    slot: thread_slot,
                    content_dirty_pending: thread_pending,
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

    struct VtActor<F>
    where
        F: Fn(VtEvent),
    {
        vt: Box<VtTerminal<'static, 'static>>,
        render_state: RenderState<'static>,
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        master: Box<dyn MasterPty + Send>,
        child: Box<dyn Child + Send + Sync>,
        commands: mpsc::Receiver<VtCommand>,
        response_rx: mpsc::Receiver<Bytes>,
        pending_writes: PendingWrites,
        notifier: ContentNotifier<F>,
        poller: Arc<Poller>,
        fd: RawFd,
        cell_width_px: u32,
        cell_height_px: u32,
        sync_gate: SyncOutputGate,
    }

    impl<F> VtActor<F>
    where
        F: Fn(VtEvent),
    {
        fn new(
            options: VtSessionOptions,
            commands: mpsc::Receiver<VtCommand>,
            poller: Arc<Poller>,
            notifier: ContentNotifier<F>,
        ) -> Result<Self, SpawnError> {
            install_png_decoder_for_this_thread();

            let mut vt = Box::new(
                VtTerminal::new(TerminalOptions {
                    cols: options.cols,
                    rows: options.rows,
                    max_scrollback: MAX_SCROLLBACK,
                })
                .map_err(|_| SpawnError::Vt("terminal new"))?,
            );
            configure_kitty_graphics(&mut vt)?;

            let render_state =
                RenderState::new().map_err(|_| SpawnError::Vt("render state new"))?;

            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: options.rows,
                    cols: options.cols,
                    pixel_width: options.pixel_width,
                    pixel_height: options.pixel_height,
                })
                .map_err(|err| SpawnError::Pty(err.to_string()))?;
            let child = pair
                .slave
                .spawn_command(CommandBuilder::new_default_prog())
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

            let (cell_width_px, cell_height_px) = cell_px(
                options.cols,
                options.rows,
                options.pixel_width,
                options.pixel_height,
            );
            vt.resize(options.cols, options.rows, cell_width_px, cell_height_px)
                .map_err(|_| SpawnError::Vt("terminal resize"))?;

            let (response_tx, response_rx) = mpsc::channel::<Bytes>();
            vt.on_pty_write(move |_, data| {
                let _ = response_tx.send(Bytes::copy_from_slice(data));
            })
            .map_err(|_| SpawnError::Vt("on pty write"))?;
            seed_cursor_shape(&mut vt, options.initial_cursor_shape);

            let mut actor = Self {
                vt,
                render_state,
                reader,
                writer,
                master: pair.master,
                child,
                commands,
                response_rx,
                pending_writes: PendingWrites::default(),
                notifier,
                poller,
                fd,
                cell_width_px,
                cell_height_px,
                sync_gate: SyncOutputGate::default(),
            };
            actor.drain_responses();
            actor
                .publish_snapshot()
                .ok_or(SpawnError::Vt("initial snapshot"))?;

            let mut interest = Event::readable(PTY_KEY);
            interest.set_interrupt(true);
            // SAFETY: `fd` belongs to `actor.master`, which is stored in the
            // actor and deleted from the poller before `master` is dropped.
            unsafe {
                actor.poller.add(fd, interest)?;
            }

            Ok(actor)
        }

        fn run(&mut self) {
            let mut events = Events::new();
            loop {
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
                    log::warn!("VT actor failed to register PTY interest: {err}");
                    self.notifier.exited();
                    break;
                }

                events.clear();
                let timeout = self.sync_gate.timeout_from(Instant::now());
                match self.poller.wait(&mut events, timeout) {
                    Ok(_) => {}
                    Err(err) => {
                        log::warn!("VT actor poll failed: {err}");
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
                            log::warn!("VT actor PTY read failed: {err}");
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
                    let _ = self.publish_snapshot();
                }
            }

            let _ = self
                .poller
                .delete(unsafe { BorrowedFd::borrow_raw(self.fd) });
            if !self.child_exited() {
                let _ = self.child.kill();
            }
        }

        fn reregister(&self) -> io::Result<()> {
            let mut interest = Event::new(PTY_KEY, true, !self.pending_writes.is_empty());
            interest.set_interrupt(true);
            self.poller
                .modify(unsafe { BorrowedFd::borrow_raw(self.fd) }, interest)
        }

        fn drain_and_apply_commands(&mut self) -> bool {
            let batch = CoalescedCommands::drain(&self.commands);
            if batch.is_empty() {
                return false;
            }
            if batch.shutdown {
                return true;
            }

            if let Some(resize) = batch.resize {
                self.apply_resize(resize);
                self.sync_gate.clear();
                let _ = self.publish_snapshot();
            }
            if let Some(theme) = batch.theme {
                apply_theme_colors(&mut self.vt, theme);
                let _ = self.publish_snapshot();
            }
            if let Some(shape) = batch.cursor_shape {
                seed_cursor_shape(&mut self.vt, shape);
                self.drain_responses();
                let _ = self.publish_snapshot();
            }
            if batch.scroll_delta != 0 {
                self.vt
                    .scroll_viewport(ScrollViewport::Delta(batch.scroll_delta as isize));
                let _ = self.publish_snapshot();
            }
            self.pending_writes.extend(batch.writes);
            false
        }

        fn apply_resize(&mut self, resize: Resize) {
            let (cell_w, cell_h) = resize.cell_pixels();
            self.cell_width_px = cell_w;
            self.cell_height_px = cell_h;
            let _ = self.vt.resize(resize.cols, resize.rows, cell_w, cell_h);
            let _ = self.master.resize(PtySize {
                rows: resize.rows,
                cols: resize.cols,
                pixel_width: resize.pixel_width,
                pixel_height: resize.pixel_height,
            });
        }

        fn flush_pending_writes(&mut self) -> io::Result<()> {
            self.pending_writes.flush(&mut self.writer)
        }

        fn read_pty_batch(&mut self) -> io::Result<ReadOutcome> {
            let mut total = 0usize;
            let mut changed = false;
            let mut buf = [0u8; READ_CHUNK];

            while total < MAX_READ_PER_TICK {
                match self.reader.read(&mut buf) {
                    Ok(0) => return Ok(ReadOutcome::Eof),
                    Ok(n) => {
                        total += n;
                        changed = true;
                        self.vt.vt_write(&buf[..n]);
                        self.drain_responses();
                    }
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                    Err(err) => return Err(err),
                }
            }

            if changed
                && self
                    .sync_gate
                    .after_parse_batch(self.sync_active(), Instant::now())
            {
                let _ = self.publish_snapshot();
            }
            Ok(ReadOutcome::Alive)
        }

        fn drain_responses(&mut self) {
            while let Ok(bytes) = self.response_rx.try_recv() {
                self.pending_writes.push(bytes);
            }
        }

        fn publish_snapshot(&mut self) -> Option<()> {
            let snapshot = build_snapshot_from_parts(
                &mut self.vt,
                &mut self.render_state,
                self.cell_width_px,
                self.cell_height_px,
            )?;
            self.notifier.publish(Arc::new(snapshot));
            Some(())
        }

        fn sync_active(&self) -> bool {
            self.vt.mode(Mode::SYNC_OUTPUT).unwrap_or(false)
        }

        fn child_exited(&mut self) -> bool {
            !matches!(self.child.try_wait(), Ok(None))
        }
    }

    enum ReadOutcome {
        Alive,
        Eof,
    }

    fn configure_kitty_graphics(
        vt: &mut Box<VtTerminal<'static, 'static>>,
    ) -> Result<(), SpawnError> {
        vt.set_kitty_image_storage_limit(KITTY_IMAGE_STORAGE_LIMIT_BYTES)
            .map_err(|_| SpawnError::Vt("kitty storage limit"))?;
        let _ = vt.set_kitty_image_from_file_allowed(true);
        let _ = vt.set_kitty_image_from_temp_file_allowed(true);
        let _ = vt.set_kitty_image_from_shared_mem_allowed(true);
        Ok(())
    }

    fn seed_cursor_shape(vt: &mut VtTerminal<'static, 'static>, shape: CursorShape) {
        vt.vt_write(cursor_shape_sequence(shape));
    }

    fn apply_theme_colors(vt: &mut VtTerminal<'static, 'static>, colors: ThemeColors) {
        let rgb = |[r, g, b]: [u8; 3]| RgbColor { r, g, b };
        let _ = vt.set_default_fg_color(Some(rgb(colors.fg)));
        let _ = vt.set_default_bg_color(Some(rgb(colors.bg)));
        let _ = vt.set_default_cursor_color(Some(rgb(colors.cursor)));
        let _ = vt.set_default_color_palette(Some(colors.palette.map(rgb)));
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
}

#[cfg(unix)]
use unix_actor::spawn_vt_session_unix;

fn cursor_shape_sequence(shape: CursorShape) -> &'static [u8] {
    match shape {
        CursorShape::Block => b"\x1b[2 q",
        CursorShape::Bar => b"\x1b[6 q",
        CursorShape::Underline => b"\x1b[4 q",
    }
}

fn cell_px(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> (u32, u32) {
    let w = if cols == 0 {
        0
    } else {
        u32::from(pixel_width) / u32::from(cols)
    };
    let h = if rows == 0 {
        0
    } else {
        u32::from(pixel_height) / u32::from(rows)
    };
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::CellColor;

    fn test_snapshot(text: &str) -> Arc<VtSnapshot> {
        let mut snapshot = VtSnapshot::empty(1, 1);
        snapshot.push_cell(
            text,
            CellColor::Default,
            CellColor::Default,
            crate::frame::CellAttrs::default(),
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
