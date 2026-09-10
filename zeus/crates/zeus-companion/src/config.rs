//! Literal bind validation and descriptor-based owner-only file access.
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CStr, CString, OsStr},
    fs::File,
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Path, PathBuf},
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

/// Paths are local-administrator configuration, never HTTP request fields.
/// Reject traversal and ambiguous spellings before doing any filesystem I/O.
fn validate_absolute_path(path: &Path) -> io::Result<()> {
    let bytes = path.as_os_str().as_bytes();
    if !bytes.starts_with(b"/") {
        return Err(invalid("absolute state path required"));
    }
    if bytes != b"/" {
        for component in bytes[1..].split(|b| *b == b'/') {
            leaf_name(OsStr::from_bytes(component))?;
        }
    }
    Ok(())
}

fn leaf_name(name: &OsStr) -> io::Result<CString> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(invalid("single normal path component required"));
    }
    CString::new(bytes).map_err(|_| invalid("NUL in state path"))
}

fn split_file_path(path: &Path) -> io::Result<(&Path, &OsStr)> {
    validate_absolute_path(path)?;
    Ok((
        path.parent().ok_or_else(|| invalid("missing parent"))?,
        path.file_name()
            .ok_or_else(|| invalid("missing filename"))?,
    ))
}

fn owned_file(fd: libc::c_int) -> io::Result<File> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: callers supply a newly opened descriptor, transferred exactly once.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

/// Anchor traversal at an open root, then resolve only validated single names.
/// O_NOFOLLOW applies to *every* component. Metadata is checked on the same
/// descriptor used for the next lookup, never by reopening the original path.
fn open_ancestors(path: &Path) -> io::Result<File> {
    validate_absolute_path(path)?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: constant NUL-terminated path; the returned descriptor is owned below.
    let mut directory = owned_file(unsafe { libc::open(c"/".as_ptr(), flags) })?;
    validate_ancestor(&directory)?;
    for name in path.as_os_str().as_bytes()[1..].split(|b| *b == b'/') {
        if name.is_empty() {
            // The root itself has no child component.
            continue;
        }
        let name = leaf_name(OsStr::from_bytes(name))?;
        // SAFETY: directory is live and name is one NUL-terminated component.
        directory =
            owned_file(unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) })?;
        validate_ancestor(&directory)?;
    }
    Ok(directory)
}

fn validate_ancestor(directory: &File) -> io::Result<()> {
    let meta = directory.metadata()?;
    let trusted_owner = meta.uid() == 0 || meta.uid() == unsafe { libc::geteuid() };
    let sticky_shared = meta.uid() == 0 && meta.mode() & 0o1000 != 0;
    if !meta.is_dir() || !trusted_owner || (meta.mode() & 0o022 != 0 && !sticky_shared) {
        return Err(invalid("unsafe state ancestor ownership or permissions"));
    }
    Ok(())
}

/// A verified owner-only directory capability. No operation reconstructs its
/// absolute path, so a renamed/replaced ancestor cannot redirect an operation.
/// The local Unix account itself remains inside the Engine trust boundary.
pub(crate) struct SecureDir(File);

impl SecureDir {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        Self::from_file(open_ancestors(path)?)
    }

    fn from_file(file: File) -> io::Result<Self> {
        let meta = file.metadata()?;
        if !meta.is_dir()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o777 != 0o700
        {
            return Err(invalid("state directory must be owner-only 0700"));
        }
        Ok(Self(file))
    }

    pub(crate) fn open_file(&self, name: &OsStr, flags: libc::c_int) -> io::Result<File> {
        self.open_leaf(&leaf_name(name)?, flags)
    }

    fn open_leaf(&self, name: &CStr, flags: libc::c_int) -> io::Result<File> {
        let file = self.open_leaf_descriptor(name, flags)?;
        validate_file(&file)?;
        Ok(file)
    }

    fn open_leaf_descriptor(&self, name: &CStr, flags: libc::c_int) -> io::Result<File> {
        // SAFETY: self owns the directory; all callers validated a single name.
        owned_file(unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY,
                0o600 as libc::c_uint,
            )
        })
    }

    pub(crate) fn read(&self, name: &OsStr, limit: usize) -> io::Result<Vec<u8>> {
        let mut file = self.open_file(name, libc::O_RDONLY)?;
        if file.metadata()?.len() > limit as u64 {
            return Err(invalid("file limit exceeded"));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take((limit as u64).saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(invalid("file limit exceeded"));
        }
        Ok(bytes)
    }

    pub(crate) fn atomic_write(&self, name: &OsStr, bytes: &[u8]) -> io::Result<()> {
        let name = leaf_name(name)?;
        match self.open_leaf(&name, libc::O_RDONLY) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let temporary = leaf_name(OsStr::new(&format!(
            ".{}.tmp",
            crate::auth::random_token()?
        )))?;
        self.publish_new_file(&name, &temporary, bytes)
    }

    fn publish_new_file(&self, name: &CStr, temporary: &CStr, bytes: &[u8]) -> io::Result<()> {
        // Do not arm cleanup until *our* exclusive creation succeeds.
        let mut file =
            self.open_leaf_descriptor(temporary, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
        let mut published = false;
        let result = (|| {
            // Creation can succeed with wrong permissions under a restrictive
            // umask. Such failures must still clean up our newly created file.
            validate_file(&file)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            // SAFETY: both single names are relative to this same owned directory.
            if unsafe {
                libc::renameat(
                    self.0.as_raw_fd(),
                    temporary.as_ptr(),
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            published = true;
            self.0.sync_all()
        })();
        if result.is_err() && !published {
            // SAFETY: remove only the nonce file successfully created above.
            unsafe {
                libc::unlinkat(self.0.as_raw_fd(), temporary.as_ptr(), 0);
            }
        }
        result
    }
}

pub fn secure_dir(path: &Path) -> io::Result<()> {
    SecureDir::open(path).map(|_| ())
}

/// Create only the final directory beneath a verified, descriptor-held parent.
pub fn create_secure_dir(path: &Path) -> io::Result<()> {
    let (parent, name) = split_file_path(path)?;
    let directory = open_ancestors(parent)?;
    let name = leaf_name(name)?;
    // SAFETY: validated single component beneath the verified directory handle.
    if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: name is still resolved relative to the same held parent.
    SecureDir::from_file(owned_file(unsafe {
        libc::openat(directory.as_raw_fd(), name.as_ptr(), flags)
    })?)
    .map(|_| ())
}

pub fn secure_read(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let (parent, name) = split_file_path(path)?;
    SecureDir::open(parent)?.read(name, limit)
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
    let (parent, name) = split_file_path(path)?;
    SecureDir::open(parent)?.atomic_write(name, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn directory() -> tempfile::TempDir {
        let temp = tempfile::tempdir_in(std::fs::canonicalize("/tmp").unwrap()).unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        temp
    }

    #[test]
    fn pinned_directory_reads_and_publishes_without_resolving_replacement_symlink() {
        let parent = directory();
        let outside = directory();
        let path = parent.path().join("state");
        create_secure_dir(&path).unwrap();
        atomic_write(&path.join("data"), b"original").unwrap();
        atomic_write(&outside.path().join("data"), b"outside sentinel").unwrap();
        let pinned = SecureDir::open(&path).unwrap();
        let moved = parent.path().join("moved");
        std::fs::rename(&path, &moved).unwrap();
        symlink(outside.path(), &path).unwrap();

        assert_eq!(pinned.read(OsStr::new("data"), 100).unwrap(), b"original");
        pinned.atomic_write(OsStr::new("data"), b"updated").unwrap();
        assert_eq!(secure_read(&moved.join("data"), 100).unwrap(), b"updated");
        assert_eq!(
            secure_read(&outside.path().join("data"), 100).unwrap(),
            b"outside sentinel"
        );
        assert!(secure_read(&path.join("data"), 100).is_err());
        assert!(atomic_write(&path.join("data"), b"must not escape").is_err());
        assert_eq!(std::fs::read_dir(&moved).unwrap().count(), 1);
    }

    #[test]
    fn rejected_paths_and_leaf_names_have_no_side_effects() {
        let parent = directory();
        let pinned = SecureDir::open(parent.path()).unwrap();
        for name in [
            "",
            ".",
            "..",
            "../outside",
            "/absolute",
            "child/file",
            "NUL\0name",
        ] {
            assert!(pinned.read(OsStr::new(name), 10).is_err());
            assert!(pinned.atomic_write(OsStr::new(name), b"no").is_err());
        }
        for path in [
            "relative".to_owned(),
            format!("{}/../escape", parent.path().display()),
            format!("{}/./child", parent.path().display()),
            format!("{}//child", parent.path().display()),
            format!("{}/child/", parent.path().display()),
        ] {
            assert!(create_secure_dir(Path::new(&path)).is_err());
            assert!(atomic_write(Path::new(&path), b"no").is_err());
        }
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
    }

    #[test]
    fn atomic_cleanup_only_removes_our_created_unpublished_file() {
        let temp = directory();
        let pinned = SecureDir::open(temp.path()).unwrap();
        pinned
            .atomic_write(OsStr::new("destination"), b"original")
            .unwrap();
        pinned
            .atomic_write(OsStr::new(".collision.tmp"), b"existing sentinel")
            .unwrap();
        assert!(
            pinned
                .publish_new_file(c"destination", c".collision.tmp", b"new")
                .is_err()
        );
        assert_eq!(
            pinned.read(OsStr::new(".collision.tmp"), 100).unwrap(),
            b"existing sentinel"
        );
        assert_eq!(
            pinned.read(OsStr::new("destination"), 100).unwrap(),
            b"original"
        );

        create_secure_dir(&temp.path().join("non-file")).unwrap();
        assert!(
            pinned
                .publish_new_file(c"non-file", c".owned.tmp", b"new")
                .is_err()
        );
        assert!(!temp.path().join(".owned.tmp").exists());
        assert!(temp.path().join("non-file").is_dir());
        assert_eq!(
            pinned.read(OsStr::new("destination"), 100).unwrap(),
            b"original"
        );
    }

    #[test]
    fn directory_and_file_handles_are_cloexec_and_reads_are_bounded() {
        let temp = directory();
        let pinned = SecureDir::open(temp.path()).unwrap();
        pinned.atomic_write(OsStr::new("data"), b"1234").unwrap();
        let file = pinned
            .open_file(OsStr::new("data"), libc::O_RDONLY)
            .unwrap();
        for fd in [pinned.0.as_raw_fd(), file.as_raw_fd()] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            assert!(flags >= 0);
            assert_ne!(flags & libc::FD_CLOEXEC, 0);
        }
        assert_eq!(pinned.read(OsStr::new("data"), 4).unwrap(), b"1234");
        assert!(pinned.read(OsStr::new("data"), 3).is_err());
        assert!(pinned.read(OsStr::new("data"), 0).is_err());
        pinned.atomic_write(OsStr::new("empty"), b"").unwrap();
        assert!(pinned.read(OsStr::new("empty"), 0).unwrap().is_empty());
    }
}
