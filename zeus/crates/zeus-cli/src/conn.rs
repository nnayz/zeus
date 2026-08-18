use std::io::{self, Read, Write};
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use zeus_proto::control::{decode_line, encode_line};
use zeus_proto::paths::{ZeusEnv, ZeusPaths};
use zeus_proto::{ControlMessage, SessionListResult, SessionRecord};

use crate::error::CliError;
use crate::support::resolve_session;

pub struct DaemonConn {
    stream: UnixStream,
    leftover: Vec<u8>,
}

impl DaemonConn {
    pub fn socket_path() -> PathBuf {
        if let Ok(path) = std::env::var(ZeusEnv::SOCKET)
            && !path.is_empty()
        {
            return PathBuf::from(path);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        ZeusPaths::socket(home)
    }

    pub fn connect() -> Result<Self, CliError> {
        Self::connect_path(&Self::socket_path(), Duration::from_secs(3))
    }

    pub fn connect_path(path: &Path, timeout: Duration) -> Result<Self, CliError> {
        let stream = connect_uds(path, timeout)
            .map_err(|error| CliError::Unreachable(format!("{}: {error}", path.display())))?;
        let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
        Ok(Self {
            stream,
            leftover: Vec::new(),
        })
    }

    pub fn request<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: &P,
        read_timeout: Duration,
    ) -> Result<R, CliError> {
        let value = self.request_value(method, params, read_timeout)?;
        serde_json::from_value(value).map_err(CliError::from)
    }

    pub fn request_value<P: Serialize>(
        &mut self,
        method: &str,
        params: &P,
        read_timeout: Duration,
    ) -> Result<Value, CliError> {
        let params = serde_json::to_value(params)?;
        self.write_request(method, Some(params))?;
        let _ = self.stream.set_read_timeout(Some(read_timeout));
        loop {
            match self.read_message()? {
                ControlMessage::Response { id: 1, result } => {
                    return result.map_err(CliError::from_control);
                }
                ControlMessage::Response {
                    result: Err(error), ..
                } => return Err(CliError::from_control(error)),
                _ => continue,
            }
        }
    }

    pub fn stream<P: Serialize>(
        &mut self,
        method: &str,
        params: &P,
        read_timeout: Option<Duration>,
        mut on_event: impl FnMut(&str, u64, &Value) -> Result<bool, CliError>,
    ) -> Result<(), CliError> {
        let params = serde_json::to_value(params)?;
        self.write_request(method, Some(params))?;
        let _ = self.stream.set_read_timeout(read_timeout);
        loop {
            match self.read_message() {
                Ok(ControlMessage::Event { name, seq, params }) => {
                    if !on_event(&name, seq, &params)? {
                        return Ok(());
                    }
                }
                Ok(ControlMessage::Response {
                    result: Err(error), ..
                }) => return Err(CliError::from_control(error)),
                Ok(ControlMessage::Response { .. } | ControlMessage::Request { .. }) => continue,
                Err(CliError::Timeout) if read_timeout.is_some() => return Err(CliError::Timeout),
                Err(CliError::Failure(message)) if message.contains("closed") => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }

    fn write_request(&mut self, method: &str, params: Option<Value>) -> Result<(), CliError> {
        let message = ControlMessage::Request {
            id: 1,
            method: method.to_string(),
            params,
        };
        self.stream.write_all(&encode_line(&message)?)?;
        self.stream.flush()?;
        Ok(())
    }

    fn read_message(&mut self) -> Result<ControlMessage, CliError> {
        let mut buffer = [0_u8; 65536];
        loop {
            if let Some(index) = self.leftover.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.leftover.drain(..=index).collect();
                if line.iter().all(|byte| matches!(byte, b'\n' | b'\r' | b' ')) {
                    continue;
                }
                return decode_line(&line).map_err(CliError::from);
            }
            match self.stream.read(&mut buffer) {
                Ok(0) => return Err(CliError::Failure("connection closed by daemon".into())),
                Ok(count) => self.leftover.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.kind() == io::ErrorKind::TimedOut =>
                {
                    return Err(CliError::Timeout);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

pub fn with_conn<T>(
    read_timeout: Duration,
    body: impl FnOnce(&mut DaemonConn) -> Result<T, CliError>,
) -> Result<T, CliError> {
    let mut conn = DaemonConn::connect()?;
    let _ = conn.stream.set_read_timeout(Some(read_timeout));
    body(&mut conn)
}

pub fn sessions() -> Result<SessionListResult, CliError> {
    with_conn(Duration::from_secs(3), |conn| {
        conn.request(
            zeus_proto::Method::SESSION_LIST,
            &zeus_proto::EmptyParams {},
            Duration::from_secs(3),
        )
    })
}

pub fn resolve(needle: &str) -> Result<SessionRecord, CliError> {
    let listed = sessions()?;
    resolve_session(needle, &listed.sessions).cloned()
}

fn connect_uds(path: &Path, timeout: Duration) -> io::Result<UnixStream> {
    let bytes = path.as_os_str().as_encoded_bytes();
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let raw = std::os::fd::AsRawFd::as_raw_fd(&owned);
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL, 0) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    if bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path too long",
        ));
    }
    for (index, byte) in bytes.iter().enumerate() {
        addr.sun_path[index] = *byte as libc::c_char;
    }
    let result = unsafe {
        libc::connect(
            raw,
            std::ptr::addr_of!(addr).cast(),
            std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
        )
    };
    if result != 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(err);
        }
        let mut pollfd = libc::pollfd {
            fd: raw,
            events: libc::POLLOUT,
            revents: 0,
        };
        let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if ready <= 0 {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "connect timed out"));
        }
        let mut so_error: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                raw,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                std::ptr::addr_of_mut!(so_error).cast(),
                &mut len,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        if so_error != 0 {
            return Err(io::Error::from_raw_os_error(so_error));
        }
    }
    unsafe {
        libc::fcntl(raw, libc::F_SETFL, flags);
    }
    Ok(unsafe { UnixStream::from_raw_fd(owned.into_raw_fd()) })
}
