//! POSIX serial port discovery and raw 8N1 I/O for SynScan hand controllers.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::types::MountError;

/// Abstract request-response serial transport for SynScan hand controllers.
pub trait SerialTransport: Send + 'static {
    fn exchange(&mut self, cmd: &[u8]) -> Result<Vec<u8>, MountError>;
}

/// Discovers candidate USB-serial devices on Linux/Unix hosts (`/dev/serial/by-id/*`,
/// `/dev/ttyUSB*`, `/dev/ttyACM*`, `/dev/cu.usb*`), deduplicating symlinks that resolve
/// to the same underlying tty device (preferring stable `/dev/serial/by-id/*` paths).
pub fn list_serial_ports() -> Vec<String> {
    let mut ports = BTreeSet::new();
    let mut seen_targets: HashSet<PathBuf> = HashSet::new();

    if let Ok(entries) = std::fs::read_dir("/dev/serial/by-id") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(path_str) = path.to_str() {
                if let Ok(canonical) = std::fs::canonicalize(&path) {
                    seen_targets.insert(canonical);
                }
                ports.insert(path_str.to_string());
            }
        }
    }

    if let Ok(entries) = std::fs::read_dir("/dev") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if name_str.starts_with("ttyUSB")
                || name_str.starts_with("ttyACM")
                || name_str.starts_with("cu.usb")
            {
                let full_path = format!("/dev/{name_str}");
                let canonical =
                    std::fs::canonicalize(&full_path).unwrap_or_else(|_| PathBuf::from(&full_path));
                if !seen_targets.contains(&canonical) {
                    ports.insert(full_path);
                }
            }
        }
    }

    ports.into_iter().collect()
}

#[cfg(unix)]
pub struct SynScanSerialPort {
    file: std::fs::File,
}

#[cfg(unix)]
impl SynScanSerialPort {
    /// Opens a serial port in raw 8N1 mode at `baud_rate` (SynScan HC default: `9600`).
    pub fn open(path: &str, baud_rate: u32) -> Result<Self, MountError> {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::io::AsRawFd;

        if !Path::new(path).exists() {
            return Err(MountError::Io(format!(
                "Serial port '{path}' does not exist"
            )));
        }

        // Open with O_NONBLOCK so we never hang waiting for Carrier Detect (DCD)
        // before CLOCAL is configured.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
            .open(path)
            .map_err(|e| MountError::Io(format!("Failed to open '{path}': {e}")))?;

        let fd = file.as_raw_fd();
        configure_termios(fd, baud_rate)?;
        clear_nonblock(fd)?;
        Ok(Self { file })
    }
}

#[cfg(unix)]
impl SerialTransport for SynScanSerialPort {
    /// Sends a SynScan command and reads bytes until the `#` terminator (or timeout).
    fn exchange(&mut self, cmd: &[u8]) -> Result<Vec<u8>, MountError> {
        use std::io::{Read, Write};
        use std::os::unix::io::AsRawFd;

        let fd = self.file.as_raw_fd();

        // Flush stale bytes from previous interrupted exchanges.
        // SAFETY: fd is a valid open serial descriptor owned by `self.file`.
        unsafe {
            libc::tcflush(fd, libc::TCIFLUSH);
        }

        self.file
            .write_all(cmd)
            .map_err(|e| MountError::Io(format!("Serial write failed: {e}")))?;
        self.file
            .flush()
            .map_err(|e| MountError::Io(format!("Serial flush failed: {e}")))?;

        let mut response = Vec::with_capacity(32);
        let mut byte = [0u8; 1];
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1200);

        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            let remaining_ms = deadline
                .saturating_duration_since(now)
                .as_millis()
                .min(i32::MAX as u128) as i32;

            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: `pfd` is a valid single-element stack struct for `fd`.
            let poll_rc = unsafe { libc::poll(&raw mut pfd, 1, remaining_ms.max(1)) };
            if poll_rc < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(MountError::Io(format!("Serial poll failed: {err}")));
            }
            if poll_rc == 0 {
                break;
            }
            if (pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)) != 0 {
                return Err(MountError::Io("Serial device disconnected".into()));
            }

            match self.file.read(&mut byte) {
                Ok(0) => {
                    return Err(MountError::Io(
                        "Serial device returned EOF (disconnected)".into(),
                    ));
                }
                Ok(_) => {
                    response.push(byte[0]);
                    if byte[0] == b'#' {
                        return Ok(response);
                    }
                    if response.len() >= 128 {
                        return Err(MountError::Protocol(
                            "Response exceeded 128 bytes without '#' terminator".into(),
                        ));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(MountError::Io(format!("Serial read failed: {e}"))),
            }
        }

        Err(MountError::Io(
            "Timed out waiting for SynScan '#' response".into(),
        ))
    }
}

#[cfg(unix)]
fn clear_nonblock(fd: std::os::unix::io::RawFd) -> Result<(), MountError> {
    // SAFETY: `fd` is a valid open file descriptor.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 {
            return Err(MountError::Io(format!(
                "fcntl(F_GETFL) failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        if libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) < 0 {
            return Err(MountError::Io(format!(
                "fcntl(F_SETFL) failed: {}",
                std::io::Error::last_os_error()
            )));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn configure_termios(fd: std::os::unix::io::RawFd, baud_rate: u32) -> Result<(), MountError> {
    let speed = match baud_rate {
        4800 => libc::B4800,
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115200 => libc::B115200,
        other => {
            return Err(MountError::Io(format!(
                "Unsupported serial baud rate: {other}"
            )));
        }
    };

    // SAFETY: `termios` is initialized by `tcgetattr` before modification.
    unsafe {
        let mut tty: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &raw mut tty) != 0 {
            return Err(MountError::Io(format!(
                "tcgetattr failed: {}",
                std::io::Error::last_os_error()
            )));
        }

        libc::cfmakeraw(&raw mut tty);
        if libc::cfsetispeed(&raw mut tty, speed) != 0
            || libc::cfsetospeed(&raw mut tty, speed) != 0
        {
            return Err(MountError::Io(format!(
                "cfset*speed failed: {}",
                std::io::Error::last_os_error()
            )));
        }

        tty.c_cflag |= libc::CLOCAL | libc::CREAD;
        tty.c_cflag &= !(libc::PARENB | libc::CSTOPB | libc::CSIZE | libc::CRTSCTS);
        tty.c_cflag |= libc::CS8;

        tty.c_cc[libc::VMIN] = 1;
        tty.c_cc[libc::VTIME] = 0;

        if libc::tcsetattr(fd, libc::TCSANOW, &raw const tty) != 0 {
            return Err(MountError::Io(format!(
                "tcsetattr failed: {}",
                std::io::Error::last_os_error()
            )));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub struct SynScanSerialPort;

#[cfg(not(unix))]
impl SynScanSerialPort {
    pub fn open(_path: &str, _baud_rate: u32) -> Result<Self, MountError> {
        Err(MountError::UnsupportedPlatform)
    }
}

#[cfg(not(unix))]
impl SerialTransport for SynScanSerialPort {
    fn exchange(&mut self, _cmd: &[u8]) -> Result<Vec<u8>, MountError> {
        Err(MountError::UnsupportedPlatform)
    }
}
