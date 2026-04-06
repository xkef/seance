//! PTY (pseudo-terminal) spawning and I/O.
//!
//! Forks a child shell process connected to a PTY pair. The parent reads
//! from / writes to the master fd; the child gets stdin/stdout/stderr
//! redirected to the slave fd.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};

use nix::pty::{openpty, Winsize};
use nix::unistd::{self, ForkResult, Pid};

#[derive(Debug, Clone, Copy)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

pub struct Pty {
    master: OwnedFd,
    child: Child,
}

enum Child {
    Active(Pid),
    Exited,
}

impl Pty {
    pub fn spawn(size: Size) -> io::Result<Self> {
        let winsize = to_winsize(size);
        let pty = openpty(&winsize, None).map_err(io_err)?;

        match unsafe { unistd::fork() }.map_err(io_err)? {
            ForkResult::Parent { child } => {
                drop(pty.slave);
                set_nonblocking(&pty.master)?;
                Ok(Self {
                    master: pty.master,
                    child: Child::Active(child),
                })
            }
            ForkResult::Child => {
                drop(pty.master);
                unistd::setsid().ok();
                setup_slave(pty.slave);

                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
                let shell_c = std::ffi::CString::new(shell.as_str()).unwrap();
                let login_arg = std::ffi::CString::new(format!(
                    "-{}",
                    shell.rsplit('/').next().unwrap_or("sh")
                ))
                .unwrap();
                unistd::execvp(&shell_c, &[login_arg]).ok();
                std::process::exit(1);
            }
        }
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        match unistd::read(&self.master, buf) {
            Ok(n) => Ok(n),
            Err(nix::errno::Errno::EAGAIN) => Ok(0),
            Err(e) => Err(io_err(e)),
        }
    }

    pub fn write_all(&self, data: &[u8]) -> io::Result<()> {
        let mut offset = 0;
        while offset < data.len() {
            match unistd::write(&self.master, &data[offset..]) {
                Ok(n) => offset += n,
                Err(nix::errno::Errno::EAGAIN) => continue,
                Err(e) => return Err(io_err(e)),
            }
        }
        Ok(())
    }

    pub fn resize(&self, size: Size) -> io::Result<()> {
        let winsize = to_winsize(size);
        let ret = unsafe {
            libc::ioctl(
                self.master.as_raw_fd(),
                libc::TIOCSWINSZ as libc::c_ulong,
                &winsize,
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn is_alive(&mut self) -> bool {
        match &self.child {
            Child::Active(pid) => {
                match nix::sys::wait::waitpid(*pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG)) {
                    Ok(nix::sys::wait::WaitStatus::StillAlive) => true,
                    _ => {
                        self.child = Child::Exited;
                        false
                    }
                }
            }
            Child::Exited => false,
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Close master fd first (handled by OwnedFd drop), then reap child.
        if let Child::Active(pid) = self.child {
            // Send SIGHUP (the standard "terminal closed" signal), then reap.
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGHUP);
            let _ = nix::sys::wait::waitpid(pid, None);
            self.child = Child::Exited;
        }
    }
}

fn setup_slave(slave: OwnedFd) {
    let raw_fd = slave.as_raw_fd();
    unsafe {
        for fd in 0..=2 {
            libc::close(fd);
        }
        libc::dup2(raw_fd, 0);
        libc::dup2(raw_fd, 1);
        libc::dup2(raw_fd, 2);
        if raw_fd > 2 {
            libc::close(raw_fd);
        }
        libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0);
    }
    std::mem::forget(slave);
}

fn set_nonblocking(fd: &OwnedFd) -> io::Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, OFlag};
    let flags = fcntl(fd, FcntlArg::F_GETFL).map_err(io_err)?;
    let flags = OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK;
    fcntl(fd, FcntlArg::F_SETFL(flags)).map_err(io_err)?;
    Ok(())
}

fn to_winsize(size: Size) -> Winsize {
    Winsize {
        ws_col: size.cols,
        ws_row: size.rows,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

fn io_err(e: nix::errno::Errno) -> io::Error {
    io::Error::from_raw_os_error(e as i32)
}
