//! Literal bind validation and descriptor-based owner-only file access.
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,
    #[serde(default)]
    pub allow_private: bool,
    pub tls_certificate: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    #[serde(default)]
    pub origins: Vec<String>,
}
fn default_bind() -> SocketAddr {
    "127.0.0.1:19773".parse().unwrap()
}
impl Default for Config {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            allow_private: false,
            tls_certificate: None,
            tls_key: None,
            origins: Vec::new(),
        }
    }
}
impl Config {
    pub fn validate(&self) -> io::Result<()> {
        validate_bind(self.bind, self.allow_private)?;
        if self.tls_certificate.is_some() != self.tls_key.is_some()
            || (!self.bind.ip().is_loopback() && self.tls_key.is_none())
        {
            return Err(invalid(
                "TLS certificate and key required for private binds",
            ));
        }
        if self.origins.len() > 8
            || self
                .origins
                .iter()
                .any(|o| o.len() > 256 || !valid_origin(o))
        {
            return Err(invalid("invalid browser origin"));
        }
        Ok(())
    }
}
fn valid_origin(o: &str) -> bool {
    validate_origin(o).is_ok()
}
pub fn validate_origin(origin: &str) -> io::Result<()> {
    let url = url::Url::parse(origin).map_err(|_| invalid("invalid origin"))?;
    let loopback = url.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if origin.len() > 256
        || url.origin().ascii_serialization() != origin
        || url.username() != ""
        || url.password().is_some()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
    {
        return Err(invalid("invalid origin"));
    }
    Ok(())
}
pub fn validate_bind(addr: SocketAddr, explicit: bool) -> io::Result<()> {
    let ip = addr.ip();
    if ip.is_loopback() {
        return Ok(());
    }
    let private = match ip {
        IpAddr::V4(v) => {
            v.is_private() || (v.octets()[0] == 100 && (64..=127).contains(&v.octets()[1]))
        }
        IpAddr::V6(v) => v.segments()[0] & 0xfe00 == 0xfc00,
    };
    if explicit && private {
        Ok(())
    } else {
        Err(invalid(
            "wildcard, public, link-local, mapped and implicit private binds refused",
        ))
    }
}
pub fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Reject symlinks and attacker-writable ancestors before reopening a path.
/// Ancestors must belong to root/current uid and exclude group/world writes,
/// except root-owned sticky shared roots (other users cannot rename our child).
/// The local Unix account itself remains inside the Engine trust boundary.
pub fn secure_dir(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(invalid("absolute state path required"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => current.push(component.as_os_str()),
            _ => return Err(invalid("noncanonical state path")),
        }
        let meta = std::fs::symlink_metadata(&current)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(invalid("unsafe state directory"));
        }
        let trusted_owner = meta.uid() == 0 || meta.uid() == unsafe { libc::geteuid() };
        let sticky_shared = meta.uid() == 0 && meta.mode() & 0o1000 != 0;
        if !trusted_owner || (meta.mode() & 0o022 != 0 && !sticky_shared) {
            return Err(invalid("unsafe state ancestor ownership or permissions"));
        }
    }
    let meta = std::fs::metadata(path)?;
    if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o777 != 0o700 {
        return Err(invalid("state directory must be owner-only 0700"));
    }
    Ok(())
}
pub fn secure_read(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    secure_dir(path.parent().ok_or_else(|| invalid("missing parent"))?)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    validate_file(&file)?;
    if file.metadata()?.len() > limit as u64 {
        return Err(invalid("file limit exceeded"));
    }
    let mut bytes = Vec::new();
    (&mut file).take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid("file limit exceeded"));
    }
    Ok(bytes)
}
pub fn validate_file(file: &File) -> io::Result<()> {
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o777 != 0o600
        || meta.nlink() != 1
    {
        return Err(invalid("state file must be regular owner-only 0600"));
    }
    Ok(())
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| invalid("missing parent"))?;
    secure_dir(parent)?;
    if path.try_exists()? {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        validate_file(&file)?;
    }
    let temporary = parent.join(format!(".{}.tmp", crate::auth::random_token()?));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
