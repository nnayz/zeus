//! Session-scoped remote image upload.
//!
//! The app stages a local file and asks the Engine to copy it onto the session
//! host. Bytes travel over the existing OpenSSH `ssh -T` channel as stdin to a
//! fixed POSIX script; the Remote Holder protocol is not involved.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Duration;

use zeus_proto::ControlError;

pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);
pub const MAX_STDOUT: usize = 4096;

/// Owner-only, session-scoped write of stdin to `$HOME/.cache/zeus/sessions/<id>/attachments`.
/// Dynamic values arrive as the first two stdin lines; they are never interpolated
/// into the command text.
pub const UPLOAD_SCRIPT: &str = r#"set -euC
umask 077
export LC_ALL=C LANG=C LANGUAGE=C
IFS= read -r session_id || exit 72
IFS= read -r file_name || exit 72
case "$session_id" in
  ""|"."|".."|*/*) exit 72 ;;
esac
case "$session_id" in
  *[!A-Za-z0-9._-]*) exit 72 ;;
esac
case "$file_name" in
  ""|"."|".."|*/*) exit 72 ;;
esac
case "$file_name" in
  *[!A-Za-z0-9._-]*) exit 72 ;;
esac
[ -d "$HOME" ] || exit 73
[ ! -L "$HOME/.cache" ] || exit 73
mkdir -p "$HOME/.cache"
[ -d "$HOME/.cache" ] && [ ! -L "$HOME/.cache" ] || exit 73
root=$HOME/.cache/zeus
sessions=$root/sessions
session=$sessions/$session_id
attachments=$session/attachments
for p in "$root" "$sessions" "$session" "$attachments"; do
  [ ! -L "$p" ] || exit 73
done
mkdir -p "$attachments"
for p in "$root" "$sessions" "$session" "$attachments"; do
  [ -d "$p" ] && [ ! -L "$p" ] || exit 73
  chmod 700 "$p"
done
temporary=$attachments/.partial-$file_name
final=$attachments/$file_name
[ ! -e "$temporary" ] && [ ! -L "$temporary" ] || exit 74
[ ! -e "$final" ] && [ ! -L "$final" ] || exit 74
trap 'rm -f "$temporary"' EXIT
cat > "$temporary"
chmod 600 "$temporary"
mv "$temporary" "$final"
trap - EXIT
printf '%s\n' "$final"
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageKind {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl ImageKind {
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }
}

#[must_use]
pub fn sniff_image(bytes: &[u8]) -> Option<ImageKind> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(ImageKind::Png);
    }
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Some(ImageKind::Jpeg);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(ImageKind::Gif);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(ImageKind::Webp);
    }
    None
}

pub fn unique_file_name(kind: ImageKind) -> Result<String, ControlError> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes)
        .map_err(|error| ControlError::internal(format!("secure random source failed: {error}")))?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("img-{hex}.{}", kind.extension()))
}

pub fn read_local_image(path: &Path) -> Result<Vec<u8>, ControlError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ControlError::bad_request("staged image is missing or unreadable"))?;
    if metadata.file_type().is_symlink() {
        return Err(ControlError::bad_request(
            "staged image must not be a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(ControlError::bad_request("staged image is not a file"));
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(ControlError::bad_request(
            "image exceeds the 20 MiB size limit",
        ));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(ControlError::bad_request("staged image is not owner-only"));
    }
    let bytes = fs::read(path)
        .map_err(|_| ControlError::bad_request("staged image is missing or unreadable"))?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(ControlError::bad_request(
            "image exceeds the 20 MiB size limit",
        ));
    }
    if sniff_image(&bytes).is_none() {
        return Err(ControlError::bad_request(
            "file is not a supported PNG, JPEG, GIF, or WebP image",
        ));
    }
    Ok(bytes)
}

pub fn upload_payload(
    session_id: &str,
    file_name: &str,
    bytes: &[u8],
) -> Result<Vec<u8>, ControlError> {
    validate_component("session id", session_id)?;
    validate_component("file name", file_name)?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(ControlError::bad_request(
            "image exceeds the 20 MiB size limit",
        ));
    }
    let mut payload = Vec::with_capacity(session_id.len() + file_name.len() + bytes.len() + 2);
    payload.extend_from_slice(session_id.as_bytes());
    payload.push(b'\n');
    payload.extend_from_slice(file_name.as_bytes());
    payload.push(b'\n');
    payload.extend_from_slice(bytes);
    Ok(payload)
}

pub fn parse_remote_path(stdout: &[u8]) -> Result<String, ControlError> {
    let path = std::str::from_utf8(stdout)
        .map_err(|_| ControlError::new("upload_failed", "remote path was not valid UTF-8"))?
        .trim();
    if path.is_empty() || path.contains('\0') || !path.starts_with('/') {
        return Err(ControlError::new(
            "upload_failed",
            "remote upload did not return an absolute path",
        ));
    }
    Ok(path.to_owned())
}

pub fn upload_failure(stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        "remote image upload failed".to_owned()
    } else {
        format!("remote image upload failed: {detail}")
    }
}

fn validate_component(field: &str, value: &str) -> Result<(), ControlError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value != "."
        && value != ".."
        && !value.contains('/');
    if valid {
        Ok(())
    } else {
        Err(ControlError::bad_request(format!(
            "{field} is not a safe path component"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC, 0x33, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn sniff_recognizes_supported_magic_and_rejects_other_bytes() {
        assert_eq!(sniff_image(PNG), Some(ImageKind::Png));
        assert_eq!(
            sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageKind::Jpeg)
        );
        assert_eq!(sniff_image(b"GIF89a...."), Some(ImageKind::Gif));
        let mut webp = b"RIFF....WEBP".to_vec();
        webp[4..8].copy_from_slice(&16u32.to_le_bytes());
        assert_eq!(sniff_image(&webp), Some(ImageKind::Webp));
        assert_eq!(sniff_image(b"%PDF-1.4"), None);
        assert_eq!(sniff_image(b""), None);
    }

    #[test]
    fn payload_keeps_image_bytes_out_of_the_command_and_rejects_metacharacters() {
        let payload = upload_payload("s_abc", "img-1.png", PNG).expect("payload");
        assert!(payload.starts_with(b"s_abc\nimg-1.png\n"));
        assert!(payload.ends_with(PNG));
        assert!(upload_payload("s_abc;rm", "img-1.png", PNG).is_err());
        assert!(upload_payload("s_abc", "../img.png", PNG).is_err());
        assert!(upload_payload("s_abc", "img-$(id).png", PNG).is_err());
    }

    #[test]
    fn read_local_image_rejects_symlinks_group_readable_files_and_non_images() {
        let dir = tempfile::tempdir().expect("temp");
        let file = dir.path().join("ok.png");
        fs::write(&file, PNG).expect("write");
        let mut permissions = fs::metadata(&file).expect("meta").permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&file, permissions).expect("chmod");
        assert_eq!(read_local_image(&file).expect("read"), PNG);

        let wide = dir.path().join("wide.png");
        fs::write(&wide, PNG).expect("write");
        let mut permissions = fs::metadata(&wide).expect("meta").permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&wide, permissions).expect("chmod");
        assert!(read_local_image(&wide).is_err());

        let text = dir.path().join("note.txt");
        fs::write(&text, b"hello").expect("write");
        let mut permissions = fs::metadata(&text).expect("meta").permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&text, permissions).expect("chmod");
        assert!(read_local_image(&text).is_err());

        let link = dir.path().join("link.png");
        std::os::unix::fs::symlink(&file, &link).expect("symlink");
        assert!(read_local_image(&link).is_err());
    }

    #[test]
    fn upload_script_writes_an_owner_only_session_scoped_file() {
        let home = tempfile::tempdir().expect("home");
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(UPLOAD_SCRIPT)
            .env("HOME", home.path())
            .env_remove("PATH")
            .env("PATH", "/bin:/usr/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sh");
        {
            let mut stdin = child.stdin.take().expect("stdin");
            stdin
                .write_all(&upload_payload("s_deadbeef", "img-aa.png", PNG).expect("payload"))
                .expect("write");
        }
        let output = child.wait_with_output().expect("wait");
        assert!(
            output.status.success(),
            "script failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let remote = parse_remote_path(&output.stdout).expect("path");
        let expected = home
            .path()
            .join(".cache/zeus/sessions/s_deadbeef/attachments/img-aa.png");
        assert_eq!(Path::new(&remote), expected);
        assert_eq!(fs::read(&expected).expect("read"), PNG);
        let mode = fs::metadata(&expected).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = fs::metadata(expected.parent().expect("parent"))
            .expect("dir")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
    }

    #[test]
    fn upload_script_rejects_unsafe_components_and_cleans_only_its_partial_file() {
        let home = tempfile::tempdir().expect("home");
        let keep = home
            .path()
            .join(".cache/zeus/sessions/s_keep/attachments/keep.png");
        fs::create_dir_all(keep.parent().expect("parent")).expect("mkdir");
        fs::write(&keep, b"keep").expect("write");

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(UPLOAD_SCRIPT)
            .env("HOME", home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sh");
        {
            let mut stdin = child.stdin.take().expect("stdin");
            stdin.write_all(b"s_keep\n../escape.png\n").expect("write");
            stdin.write_all(PNG).expect("write");
        }
        let output = child.wait_with_output().expect("wait");
        assert!(!output.status.success());
        assert_eq!(fs::read(&keep).expect("kept"), b"keep");
    }
}
