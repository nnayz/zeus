//! Shared staging for clipboard paste and drag-and-drop image attachments.

use std::fs;
use std::io::{Cursor, Write as _};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use image::{GenericImageView, ImageFormat, ImageReader};
use tempfile::NamedTempFile;
use zeus_proto::{AgentDescriptor, ImageAttachmentCapability};

pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_IMAGE_DIMENSION: u32 = 8192;
const MAX_RETAINED_FILES: usize = 32;
const MAX_RETAINED_BYTES: u64 = 128 * 1024 * 1024;

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

    fn from_format(format: ImageFormat) -> Option<Self> {
        match format {
            ImageFormat::Png => Some(Self::Png),
            ImageFormat::Jpeg => Some(Self::Jpeg),
            ImageFormat::Gif => Some(Self::Gif),
            ImageFormat::WebP => Some(Self::Webp),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum StageError {
    UnsupportedType,
    Directory,
    TooLarge,
    TooManyPixels,
    Unreadable,
}

impl StageError {
    #[must_use]
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::UnsupportedType => "Only PNG, JPEG, GIF, and WebP images can be attached",
            Self::Directory => "Folders cannot be attached",
            Self::TooLarge => "Image exceeds the 20 MiB size limit",
            Self::TooManyPixels => "Image exceeds the 8192px dimension limit",
            Self::Unreadable => "Could not read the image",
        }
    }
}

#[derive(Debug)]
pub struct StagedImage {
    file: NamedTempFile,
    pub original_name: String,
    pub bytes: u64,
}

impl StagedImage {
    #[must_use]
    pub fn path(&self) -> &Path {
        self.file.path()
    }

    #[must_use]
    pub fn path_string(&self) -> String {
        self.path().to_string_lossy().into_owned()
    }
}

#[derive(Debug, Default)]
pub struct ImageStore {
    files: Vec<StagedImage>,
}

impl ImageStore {
    pub fn retain(&mut self, image: StagedImage) {
        self.files.push(image);
        while self.files.len() > MAX_RETAINED_FILES || self.total_bytes() > MAX_RETAINED_BYTES {
            if self.files.len() <= 1 {
                break;
            }
            self.files.remove(0);
        }
    }

    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|image| image.bytes).sum()
    }

    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentDecision {
    Unsupported { message: String },
    Rejected { message: String },
    Ready { display_names: Vec<String> },
}

#[must_use]
pub fn capability_from_descriptor(
    descriptor: Option<&AgentDescriptor>,
) -> Option<&ImageAttachmentCapability> {
    descriptor.and_then(AgentDescriptor::image_path_capability)
}

#[must_use]
pub fn unsupported_message(display_name: &str) -> String {
    format!("{display_name} does not accept image attachments")
}

pub fn stage_bytes(
    bytes: &[u8],
    original_name: impl Into<String>,
) -> Result<StagedImage, StageError> {
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(StageError::TooLarge);
    }
    let format = image::guess_format(bytes).map_err(|_| StageError::UnsupportedType)?;
    let kind = ImageKind::from_format(format).ok_or(StageError::UnsupportedType)?;
    let decoded = ImageReader::with_format(Cursor::new(bytes), format)
        .decode()
        .map_err(|_| StageError::Unreadable)?;
    let (width, height) = decoded.dimensions();
    if width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(StageError::TooManyPixels);
    }
    write_staged(bytes, kind, original_name.into())
}

pub fn stage_path(path: &Path) -> Result<StagedImage, StageError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| StageError::Unreadable)?;
    if metadata.file_type().is_symlink() {
        return Err(StageError::Unreadable);
    }
    if metadata.is_dir() {
        return Err(StageError::Directory);
    }
    if !metadata.is_file() {
        return Err(StageError::UnsupportedType);
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(StageError::TooLarge);
    }
    let bytes = fs::read(path).map_err(|_| StageError::Unreadable)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".into());
    stage_bytes(&bytes, name)
}

pub fn decide_drop(descriptor: Option<&AgentDescriptor>, paths: &[PathBuf]) -> AttachmentDecision {
    let Some(capability) = capability_from_descriptor(descriptor) else {
        let name = descriptor
            .map(|descriptor| descriptor.display_name.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("This agent");
        return AttachmentDecision::Unsupported {
            message: unsupported_message(name),
        };
    };
    if paths.is_empty() {
        return AttachmentDecision::Rejected {
            message: StageError::Unreadable.user_message().to_owned(),
        };
    }
    let selected = if capability.multiple {
        paths
    } else {
        paths.get(..1).unwrap_or(&[])
    };
    AttachmentDecision::Ready {
        display_names: selected
            .iter()
            .map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "image".into())
            })
            .collect(),
    }
}

pub fn stage_drop(
    descriptor: Option<&AgentDescriptor>,
    paths: &[PathBuf],
) -> Result<Vec<StagedImage>, AttachmentDecision> {
    match decide_drop(descriptor, paths) {
        AttachmentDecision::Ready { .. } => {}
        other => return Err(other),
    }
    let capability = capability_from_descriptor(descriptor).expect("checked");
    let selected = if capability.multiple {
        paths.to_vec()
    } else {
        paths.iter().take(1).cloned().collect()
    };
    let mut staged = Vec::new();
    for path in &selected {
        match stage_path(path) {
            Ok(image) => staged.push(image),
            Err(error) => {
                return Err(AttachmentDecision::Rejected {
                    message: error.user_message().to_owned(),
                });
            }
        }
    }
    if staged.is_empty() {
        return Err(AttachmentDecision::Rejected {
            message: StageError::UnsupportedType.user_message().to_owned(),
        });
    }
    Ok(staged)
}

pub fn paste_paths(paths: &[String]) -> String {
    paths.join(" ")
}

pub fn keep_staged(store: &mut ImageStore, images: Vec<StagedImage>) -> Vec<String> {
    let mut paths = Vec::with_capacity(images.len());
    for image in images {
        paths.push(image.path_string());
        store.retain(image);
    }
    paths
}

fn write_staged(
    bytes: &[u8],
    kind: ImageKind,
    original_name: String,
) -> Result<StagedImage, StageError> {
    let mut file = tempfile::Builder::new()
        .prefix("zeus-img-")
        .suffix(&format!(".{}", kind.extension()))
        .tempfile()
        .map_err(|_| StageError::Unreadable)?;
    file.write_all(bytes).map_err(|_| StageError::Unreadable)?;
    file.flush().map_err(|_| StageError::Unreadable)?;
    let mut permissions = file
        .as_file()
        .metadata()
        .map_err(|_| StageError::Unreadable)?
        .permissions();
    permissions.set_mode(0o600);
    file.as_file()
        .set_permissions(permissions)
        .map_err(|_| StageError::Unreadable)?;
    Ok(StagedImage {
        file,
        original_name,
        bytes: bytes.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zeus_proto::AgentDescriptor;

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(width, height, image::Rgb([12, 34, 56]));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("encode png");
        bytes
    }

    fn supporting_agent(multiple: bool) -> AgentDescriptor {
        AgentDescriptor {
            id: "claude-code".into(),
            display_name: "Claude Code".into(),
            attachments: Some(zeus_proto::AgentAttachments {
                images: Some(ImageAttachmentCapability {
                    strategy: ImageAttachmentCapability::PATH_STRATEGY.into(),
                    multiple,
                }),
            }),
            ..AgentDescriptor::default()
        }
    }

    #[test]
    fn clipboard_bytes_and_dropped_files_share_staging() {
        let bytes = png_bytes(8, 8);
        let from_bytes = stage_bytes(&bytes, "clip.png").expect("bytes");
        assert_eq!(fs::read(from_bytes.path()).unwrap(), bytes);
        assert!(from_bytes.path().to_string_lossy().ends_with(".png"));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo from finder.png");
        fs::write(&path, &bytes).unwrap();
        let from_path = stage_path(&path).expect("path");
        assert_eq!(fs::read(from_path.path()).unwrap(), bytes);
        assert_eq!(from_path.original_name, "photo from finder.png");
    }

    #[test]
    fn unsupported_formats_and_oversized_images_are_rejected() {
        assert!(matches!(
            stage_bytes(b"%PDF-1.4", "doc.pdf"),
            Err(StageError::UnsupportedType)
        ));
        let huge = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
            .into_iter()
            .chain(vec![0; (MAX_IMAGE_BYTES as usize) + 1])
            .collect::<Vec<_>>();
        assert!(matches!(
            stage_bytes(&huge, "huge.png"),
            Err(StageError::TooLarge)
        ));

        let wide = png_bytes(MAX_IMAGE_DIMENSION + 1, 8);
        assert!(matches!(
            stage_bytes(&wide, "wide.png"),
            Err(StageError::TooManyPixels)
        ));
    }

    #[test]
    fn directories_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(stage_path(dir.path()), Err(StageError::Directory)));
    }

    #[test]
    fn agent_without_capability_receives_no_staged_files() {
        let shell = AgentDescriptor {
            id: "shell".into(),
            display_name: "Shell".into(),
            ..AgentDescriptor::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.png");
        fs::write(&path, png_bytes(4, 4)).unwrap();
        let outcome = stage_drop(Some(&shell), &[path]);
        assert!(matches!(
            outcome,
            Err(AttachmentDecision::Unsupported { .. })
        ));
    }

    #[test]
    fn multiple_images_preserve_drop_order_and_unsafe_names_stay_as_paths() {
        let agent = supporting_agent(true);
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a b.png");
        let second = dir.path().join("quote\".png");
        let third = dir.path().join("semi;rm.png");
        fs::write(&first, png_bytes(2, 2)).unwrap();
        fs::write(&second, png_bytes(3, 3)).unwrap();
        fs::write(&third, png_bytes(4, 4)).unwrap();
        let mut store = ImageStore::default();
        let staged = stage_drop(Some(&agent), &[first, second, third]).expect("stage");
        let names: Vec<_> = staged
            .iter()
            .map(|image| image.original_name.clone())
            .collect();
        assert_eq!(names, ["a b.png", "quote\".png", "semi;rm.png"]);
        let paths = keep_staged(&mut store, staged);
        let payload = paste_paths(&paths);
        assert!(!payload.contains(';'));
        assert_eq!(paths.len(), 3);
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn single_capability_keeps_only_the_first_dropped_file() {
        let agent = supporting_agent(false);
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("one.png");
        let second = dir.path().join("two.png");
        fs::write(&first, png_bytes(2, 2)).unwrap();
        fs::write(&second, png_bytes(2, 2)).unwrap();
        match decide_drop(Some(&agent), &[first, second]) {
            AttachmentDecision::Ready { display_names } => {
                assert_eq!(display_names, ["one.png"]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn store_evicts_oldest_files_when_the_bound_is_exceeded() {
        let mut store = ImageStore::default();
        for index in 0..(MAX_RETAINED_FILES + 3) {
            let staged = stage_bytes(&png_bytes(2, 2), format!("{index}.png")).unwrap();
            store.retain(staged);
        }
        assert_eq!(store.len(), MAX_RETAINED_FILES);
    }
}
