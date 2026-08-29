//! Native local-window screenshots for the Zeus MCP tool.
//!
//! ScreenCaptureKit can block while macOS decides Screen Recording permission,
//! and `scap` exposes its next frame as a blocking receive. Production capture
//! therefore runs in a short-lived child of the app-launched Engine. The
//! Engine bounds that process and kills it on timeout, which also guarantees a
//! stuck capturer cannot leave the recording indicator or stream running.

use std::fmt;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const JPEG_QUALITY: u8 = 60;
const MAX_IMAGE_EDGE: u32 = 1_280;
const MAX_JPEG_BYTES: usize = 2_500_000;
#[cfg(target_os = "macos")]
const MAX_WORKER_OUTPUT_BYTES: usize = 3_750_000;
#[cfg(target_os = "macos")]
const WORKER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Private argument used only when the Engine launches its bounded capture
/// worker. It is not a user-facing CLI command.
pub const WORKER_ARGUMENT: &str = "--zeus-screenshot-worker";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct ScreenshotRequest {
    window: Option<String>,
    display: Option<u32>,
    list: bool,
}

impl ScreenshotRequest {
    fn parse(arguments: &Value) -> Result<Self, ScreenshotError> {
        let request: Self = serde_json::from_value(arguments.clone()).map_err(|error| {
            ScreenshotError::new(
                "bad_request",
                format!("invalid screenshot arguments: {error}"),
            )
        })?;
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> Result<(), ScreenshotError> {
        if self.window.is_some() && self.display.is_some() {
            return Err(ScreenshotError::new(
                "bad_request",
                "`window` and `display` are mutually exclusive",
            ));
        }
        if self
            .window
            .as_deref()
            .is_some_and(|window| window.trim().is_empty())
        {
            return Err(ScreenshotError::new(
                "bad_request",
                "`window` must contain a non-empty title substring",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ScreenshotError {
    pub code: String,
    pub message: String,
}

impl ScreenshotError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    fn unsupported() -> Self {
        Self::new(
            "screen_capture_unsupported",
            "native screenshots are supported only by the local macOS Zeus app",
        )
    }

    fn permission_denied(message: impl Into<String>) -> Self {
        Self::new("screen_recording_denied", message)
    }

    fn capture_failed(message: impl Into<String>) -> Self {
        Self::new("screenshot_failed", message)
    }
}

impl fmt::Display for ScreenshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ScreenshotError {}

/// Captures one local target, or lists targets when `list` is true.
///
/// On macOS this delegates to a bounded child of the Engine so a permission
/// dialog or a stalled first frame can never hang the MCP control connection.
pub fn capture(arguments: &Value) -> Result<Value, ScreenshotError> {
    let request = ScreenshotRequest::parse(arguments)?;

    #[cfg(target_os = "macos")]
    {
        capture_via_worker(&request)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = request;
        Err(ScreenshotError::unsupported())
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkerEnvelope {
    #[serde(skip_serializing_if = "Option::is_none")]
    ok: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ScreenshotError>,
}

impl WorkerEnvelope {
    fn from_result(result: Result<Value, ScreenshotError>) -> Self {
        match result {
            Ok(value) => Self {
                ok: Some(value),
                error: None,
            },
            Err(error) => Self {
                ok: None,
                error: Some(error),
            },
        }
    }
}

/// Handles the private capture-worker mode before normal daemon startup.
/// Returns true when the current process was a worker and normal startup must
/// stop. The worker writes pixels only to stdout; image bytes are never logged.
pub fn run_worker_if_requested() -> bool {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new(WORKER_ARGUMENT)) {
        return false;
    }

    let result = std::panic::catch_unwind(|| {
        let request: ScreenshotRequest =
            serde_json::from_reader(std::io::Read::take(std::io::stdin().lock(), 64 * 1024))
                .map_err(|error| {
                    ScreenshotError::new("bad_request", format!("invalid worker request: {error}"))
                })?;
        request.validate()?;
        capture_in_process(&request)
    })
    .unwrap_or_else(|_| {
        Err(ScreenshotError::capture_failed(
            "native capture stopped unexpectedly",
        ))
    });

    let envelope = WorkerEnvelope::from_result(result);
    let _ = serde_json::to_writer(std::io::stdout().lock(), &envelope);
    true
}

#[cfg(target_os = "macos")]
fn capture_via_worker(request: &ScreenshotRequest) -> Result<Value, ScreenshotError> {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let executable = std::env::current_exe().map_err(|error| {
        ScreenshotError::capture_failed(format!("could not locate the Engine helper: {error}"))
    })?;
    let mut child = Command::new(executable)
        .arg(WORKER_ARGUMENT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            ScreenshotError::capture_failed(format!("could not launch capture worker: {error}"))
        })?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ScreenshotError::capture_failed("capture worker stdin was unavailable"))?;
    serde_json::to_writer(&mut stdin, request).map_err(|error| {
        ScreenshotError::capture_failed(format!("could not send capture request: {error}"))
    })?;
    stdin.flush().map_err(|error| {
        ScreenshotError::capture_failed(format!("could not send capture request: {error}"))
    })?;
    drop(stdin);

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ScreenshotError::capture_failed("capture worker stdout was unavailable"))?;
    let output_reader = match std::thread::Builder::new()
        .name("zeus-screenshot-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(MAX_WORKER_OUTPUT_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        }) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ScreenshotError::capture_failed(format!(
                "could not read capture result: {error}"
            )));
        }
    };

    let deadline = Instant::now() + WORKER_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ScreenshotError::capture_failed(format!(
                    "could not monitor capture worker: {error}"
                )));
            }
        }
    };

    let bytes = output_reader
        .join()
        .map_err(|_| ScreenshotError::capture_failed("capture output reader stopped"))?
        .map_err(|error| {
            ScreenshotError::capture_failed(format!("could not read capture result: {error}"))
        })?;

    let Some(status) = status else {
        return if scap::has_permission() {
            Err(ScreenshotError::new(
                "screenshot_timeout",
                "native capture did not produce a frame within 15 seconds",
            ))
        } else {
            Err(ScreenshotError::permission_denied(
                "Screen Recording permission was not granted within 15 seconds. Ask the user to enable Screen Recording for Zeus in System Settings > Privacy & Security.",
            ))
        };
    };
    if bytes.len() > MAX_WORKER_OUTPUT_BYTES {
        return Err(ScreenshotError::capture_failed(
            "capture worker returned an oversized image",
        ));
    }

    let envelope: WorkerEnvelope = serde_json::from_slice(&bytes).map_err(|error| {
        ScreenshotError::capture_failed(format!(
            "capture worker returned an invalid response (status {status}): {error}"
        ))
    })?;
    match (envelope.ok, envelope.error) {
        (Some(value), None) => Ok(value),
        (None, Some(error)) => Err(error),
        _ => Err(ScreenshotError::capture_failed(
            "capture worker returned an incomplete response",
        )),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetKind {
    Window,
    Display,
}

impl TargetKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Display => "display",
        }
    }
}

#[derive(Clone, Debug)]
struct TargetInfo {
    kind: TargetKind,
    id: u32,
    title: String,
    owner_bundle: Option<String>,
    active: bool,
    on_screen: bool,
}

#[derive(Clone, Debug)]
struct RawFrame {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

trait CaptureBackend {
    fn is_supported(&self) -> bool;
    fn has_permission(&self) -> bool;
    fn request_permission(&mut self) -> bool;
    fn targets(&mut self) -> Result<Vec<TargetInfo>, ScreenshotError>;
    fn capture(&mut self, target: &TargetInfo) -> Result<RawFrame, ScreenshotError>;
}

fn capture_with_backend(
    request: &ScreenshotRequest,
    backend: &mut impl CaptureBackend,
) -> Result<Value, ScreenshotError> {
    if !backend.is_supported() {
        return Err(ScreenshotError::unsupported());
    }
    if !backend.has_permission() {
        let granted = backend.request_permission();
        if !granted || !backend.has_permission() {
            return Err(ScreenshotError::permission_denied(
                "Screen Recording permission was denied. Ask the user to enable Screen Recording for Zeus in System Settings > Privacy & Security.",
            ));
        }
    }

    let targets = backend.targets()?;
    if request.list {
        return Ok(json!({
            "targets": targets.iter().map(|target| json!({
                "id": target.id,
                "title": target.title,
                "kind": target.kind.as_str(),
            })).collect::<Vec<_>>()
        }));
    }

    let resolved = resolve_target(request, &targets)?;
    let frame = backend.capture(resolved.target)?;
    let encoded = encode_bgra_as_jpeg(frame)?;
    if encoded.jpeg.len() > MAX_JPEG_BYTES {
        return Err(ScreenshotError::capture_failed(format!(
            "JPEG exceeded the {} byte screenshot limit",
            MAX_JPEG_BYTES
        )));
    }

    Ok(json!({
        "target": resolved.label,
        "targetId": resolved.target.id,
        "targetTitle": resolved.target.title,
        "width": encoded.width,
        "height": encoded.height,
        "mimeType": "image/jpeg",
        "data": base64::engine::general_purpose::STANDARD.encode(encoded.jpeg),
    }))
}

struct ResolvedTarget<'a> {
    target: &'a TargetInfo,
    label: String,
}

fn resolve_target<'a>(
    request: &ScreenshotRequest,
    targets: &'a [TargetInfo],
) -> Result<ResolvedTarget<'a>, ScreenshotError> {
    if let Some(display_id) = request.display {
        let target = targets
            .iter()
            .find(|target| target.kind == TargetKind::Display && target.id == display_id)
            .ok_or_else(|| {
                ScreenshotError::new(
                    "screenshot_target_not_found",
                    format!(
                        "display {display_id} is not capturable; call screenshot with list=true"
                    ),
                )
            })?;
        return Ok(ResolvedTarget {
            target,
            label: format!("display:{display_id}"),
        });
    }

    if let Some(window) = request.window.as_deref() {
        let needle = window.trim().to_lowercase();
        let matches: Vec<&TargetInfo> = targets
            .iter()
            .filter(|target| {
                target.kind == TargetKind::Window && target.title.to_lowercase().contains(&needle)
            })
            .collect();
        let exact: Vec<&TargetInfo> = matches
            .iter()
            .copied()
            .filter(|target| target.title.eq_ignore_ascii_case(window.trim()))
            .collect();
        let target = match (exact.as_slice(), matches.as_slice()) {
            ([target], _) => *target,
            ([], [target]) => *target,
            ([], []) => {
                return Err(ScreenshotError::new(
                    "screenshot_target_not_found",
                    format!(
                        "no capturable window title contains {window:?}; call screenshot with list=true"
                    ),
                ));
            }
            _ => {
                let candidates = matches
                    .iter()
                    .map(|target| format!("{} ({})", target.title, target.id))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(ScreenshotError::new(
                    "screenshot_target_ambiguous",
                    format!("window title {window:?} matched multiple targets: {candidates}"),
                ));
            }
        };
        return Ok(ResolvedTarget {
            target,
            label: format!("window:{}", target.id),
        });
    }

    let candidates: Vec<&TargetInfo> = targets
        .iter()
        .filter(|target| {
            target.kind == TargetKind::Window
                && target.on_screen
                && target.owner_bundle.as_deref().is_some_and(is_zeus_bundle)
        })
        .collect();
    let target = match candidates.as_slice() {
        [] => {
            return Err(ScreenshotError::new(
                "zeus_window_unavailable",
                "no local Zeus window is capturable. This screenshot tool captures the local Mac, not a remote host.",
            ));
        }
        [target] => *target,
        _ => {
            let active: Vec<&TargetInfo> = candidates
                .iter()
                .copied()
                .filter(|target| target.active)
                .collect();
            let [target] = active.as_slice() else {
                return Err(ScreenshotError::new(
                    "zeus_window_ambiguous",
                    "multiple local Zeus windows are capturable and no single one is active; call screenshot with list=true, then pass window",
                ));
            };
            *target
        }
    };
    Ok(ResolvedTarget {
        target,
        label: "zeus".to_owned(),
    })
}

fn is_zeus_bundle(bundle: &str) -> bool {
    bundle == "com.zeus.zeus" || bundle.starts_with("com.zeus.zeus.")
}

struct EncodedImage {
    width: u32,
    height: u32,
    jpeg: Vec<u8>,
}

fn encode_bgra_as_jpeg(frame: RawFrame) -> Result<EncodedImage, ScreenshotError> {
    let pixels = u64::from(frame.width)
        .checked_mul(u64::from(frame.height))
        .ok_or_else(|| ScreenshotError::capture_failed("captured frame dimensions overflow"))?;
    let expected = pixels
        .checked_mul(4)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(|| ScreenshotError::capture_failed("captured frame is too large"))?;
    if frame.width == 0 || frame.height == 0 || frame.bgra.len() != expected {
        return Err(ScreenshotError::capture_failed(format!(
            "captured BGRA frame has invalid dimensions or byte length ({}x{}, {} bytes)",
            frame.width,
            frame.height,
            frame.bgra.len()
        )));
    }

    let mut rgb = Vec::with_capacity(expected / 4 * 3);
    for pixel in frame.bgra.as_chunks::<4>().0 {
        rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
    }
    let image = image::RgbImage::from_raw(frame.width, frame.height, rgb)
        .ok_or_else(|| ScreenshotError::capture_failed("could not construct captured image"))?;
    let (width, height, rgb) = if frame.width.max(frame.height) > MAX_IMAGE_EDGE {
        let scale = MAX_IMAGE_EDGE as f64 / f64::from(frame.width.max(frame.height));
        let width = (f64::from(frame.width) * scale).round().max(1.0) as u32;
        let height = (f64::from(frame.height) * scale).round().max(1.0) as u32;
        let resized =
            image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle);
        (width, height, resized.into_raw())
    } else {
        (frame.width, frame.height, image.into_raw())
    };

    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
        .encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .map_err(|error| {
            ScreenshotError::capture_failed(format!("could not encode JPEG: {error}"))
        })?;
    Ok(EncodedImage {
        width,
        height,
        jpeg,
    })
}

#[cfg(target_os = "macos")]
fn capture_in_process(request: &ScreenshotRequest) -> Result<Value, ScreenshotError> {
    let mut backend = NativeBackend::default();
    capture_with_backend(request, &mut backend)
}

#[cfg(not(target_os = "macos"))]
fn capture_in_process(_request: &ScreenshotRequest) -> Result<Value, ScreenshotError> {
    Err(ScreenshotError::unsupported())
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct NativeBackend {
    targets: Vec<(TargetInfo, scap::Target)>,
}

#[cfg(target_os = "macos")]
impl CaptureBackend for NativeBackend {
    fn is_supported(&self) -> bool {
        scap::is_supported()
    }

    fn has_permission(&self) -> bool {
        scap::has_permission()
    }

    fn request_permission(&mut self) -> bool {
        scap::request_permission()
    }

    fn targets(&mut self) -> Result<Vec<TargetInfo>, ScreenshotError> {
        use std::collections::HashMap;

        let content = futures::executor::block_on(cidre::sc::ShareableContent::current()).map_err(
            |error| {
                ScreenshotError::capture_failed(format!(
                    "could not enumerate ScreenCaptureKit content: {error:?}"
                ))
            },
        )?;
        let window_owners: HashMap<u32, (Option<String>, bool, bool)> = content
            .windows()
            .iter()
            .map(|window| {
                let owner_bundle = window
                    .owning_app()
                    .map(|application| application.bundle_id().to_string());
                (
                    window.id(),
                    (owner_bundle, window.is_active(), window.is_on_screen()),
                )
            })
            .collect();

        self.targets = scap::get_all_targets()
            .into_iter()
            .map(|target| {
                let info = match &target {
                    scap::Target::Window(window) => {
                        let (owner_bundle, active, on_screen) = window_owners
                            .get(&window.id)
                            .cloned()
                            .unwrap_or((None, false, false));
                        TargetInfo {
                            kind: TargetKind::Window,
                            id: window.id,
                            title: window.title.clone(),
                            owner_bundle,
                            active,
                            on_screen,
                        }
                    }
                    scap::Target::Display(display) => TargetInfo {
                        kind: TargetKind::Display,
                        id: display.id,
                        title: display.title.clone(),
                        owner_bundle: None,
                        active: false,
                        on_screen: true,
                    },
                };
                (info, target)
            })
            .collect();
        Ok(self.targets.iter().map(|(info, _)| info.clone()).collect())
    }

    fn capture(&mut self, target: &TargetInfo) -> Result<RawFrame, ScreenshotError> {
        use scap::capturer::{Capturer, Options, Resolution};
        use scap::frame::{Frame, FrameType, VideoFrame};

        let native_target = self
            .targets
            .iter()
            .find(|(candidate, _)| candidate.kind == target.kind && candidate.id == target.id)
            .map(|(_, target)| target.clone())
            .ok_or_else(|| {
                ScreenshotError::new(
                    "screenshot_target_not_found",
                    "the selected target disappeared before capture",
                )
            })?;
        let options = Options {
            fps: 1,
            show_cursor: false,
            show_highlight: false,
            target: Some(native_target),
            output_type: FrameType::BGRAFrame,
            output_resolution: Resolution::_720p,
            captures_audio: false,
            exclude_current_process_audio: true,
            ..Default::default()
        };
        let mut capturer = Capturer::build(options).map_err(|error| match error {
            scap::capturer::CapturerBuildError::NotSupported => ScreenshotError::unsupported(),
            scap::capturer::CapturerBuildError::PermissionNotGranted => {
                ScreenshotError::permission_denied(
                    "Screen Recording permission is not granted for Zeus",
                )
            }
        })?;

        capturer.start_capture();
        let frame = (|| {
            for _ in 0..3 {
                match capturer.get_next_frame().map_err(|error| {
                    ScreenshotError::capture_failed(format!(
                        "native capture ended before a frame arrived: {error}"
                    ))
                })? {
                    Frame::Video(VideoFrame::BGRA(frame))
                        if frame.width > 0 && frame.height > 0 && !frame.data.is_empty() =>
                    {
                        return Ok(RawFrame {
                            width: frame.width as u32,
                            height: frame.height as u32,
                            bgra: frame.data,
                        });
                    }
                    Frame::Video(VideoFrame::BGRA(_)) => continue,
                    Frame::Video(_) => {
                        return Err(ScreenshotError::capture_failed(
                            "native capturer returned a non-BGRA video frame",
                        ));
                    }
                    Frame::Audio(_) => continue,
                }
            }
            Err(ScreenshotError::capture_failed(
                "native capturer returned only blank frames",
            ))
        })();
        capturer.stop_capture();
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeBackend {
        supported: bool,
        permitted: bool,
        grants_permission: bool,
        targets: Vec<TargetInfo>,
        frame: Option<RawFrame>,
        permission_requests: usize,
        captures: usize,
    }

    impl CaptureBackend for FakeBackend {
        fn is_supported(&self) -> bool {
            self.supported
        }

        fn has_permission(&self) -> bool {
            self.permitted
        }

        fn request_permission(&mut self) -> bool {
            self.permission_requests += 1;
            self.permitted = self.grants_permission;
            self.grants_permission
        }

        fn targets(&mut self) -> Result<Vec<TargetInfo>, ScreenshotError> {
            Ok(self.targets.clone())
        }

        fn capture(&mut self, _target: &TargetInfo) -> Result<RawFrame, ScreenshotError> {
            self.captures += 1;
            self.frame
                .take()
                .ok_or_else(|| ScreenshotError::capture_failed("fake has no frame"))
        }
    }

    fn target(kind: TargetKind, id: u32, title: &str, owner_bundle: Option<&str>) -> TargetInfo {
        TargetInfo {
            kind,
            id,
            title: title.to_owned(),
            owner_bundle: owner_bundle.map(str::to_owned),
            active: false,
            on_screen: true,
        }
    }

    fn backend() -> FakeBackend {
        FakeBackend {
            supported: true,
            permitted: true,
            grants_permission: false,
            targets: vec![
                target(TargetKind::Display, 9, "Main Display", None),
                target(TargetKind::Window, 10, "Other", Some("com.example.other")),
                target(TargetKind::Window, 11, "Zeus", Some("com.zeus.zeus")),
            ],
            frame: Some(RawFrame {
                width: 2,
                height: 1,
                bgra: vec![0, 0, 255, 255, 0, 255, 0, 255],
            }),
            permission_requests: 0,
            captures: 0,
        }
    }

    #[test]
    fn default_capture_selects_a_zeus_window_and_encodes_jpeg() {
        let mut backend = backend();
        let result =
            capture_with_backend(&ScreenshotRequest::default(), &mut backend).expect("screenshot");

        assert_eq!(result["target"], "zeus");
        assert_eq!(result["targetId"], 11);
        assert_eq!(result["mimeType"], "image/jpeg");
        assert_eq!(backend.captures, 1, "the display must not be captured");
        let jpeg = base64::engine::general_purpose::STANDARD
            .decode(result["data"].as_str().expect("base64"))
            .expect("valid base64");
        let decoded = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
            .expect("valid JPEG");
        assert_eq!((decoded.width(), decoded.height()), (2, 1));
    }

    #[test]
    fn list_returns_titles_and_ids_without_encoding_an_image() {
        let mut backend = backend();
        let result = capture_with_backend(
            &ScreenshotRequest {
                list: true,
                ..Default::default()
            },
            &mut backend,
        )
        .expect("target list");

        assert_eq!(result["targets"].as_array().expect("targets").len(), 3);
        assert!(result.get("data").is_none());
        assert_eq!(backend.captures, 0);
    }

    #[test]
    fn window_and_display_together_are_a_bad_request() {
        let error = ScreenshotRequest::parse(&json!({"window":"Zeus","display":9}))
            .expect_err("mutually exclusive");
        assert_eq!(error.code, "bad_request");
    }

    #[test]
    fn denied_permission_is_structured_and_never_captures() {
        let mut backend = backend();
        backend.permitted = false;
        let error =
            capture_with_backend(&ScreenshotRequest::default(), &mut backend).expect_err("denied");

        assert_eq!(error.code, "screen_recording_denied");
        assert_eq!(backend.permission_requests, 1);
        assert_eq!(backend.captures, 0);
    }

    #[test]
    fn newly_granted_permission_continues_to_capture() {
        let mut backend = backend();
        backend.permitted = false;
        backend.grants_permission = true;
        let result = capture_with_backend(&ScreenshotRequest::default(), &mut backend)
            .expect("permission grant");

        assert_eq!(result["mimeType"], "image/jpeg");
        assert_eq!(backend.permission_requests, 1);
        assert_eq!(backend.captures, 1);
    }

    #[test]
    fn title_substrings_must_resolve_unambiguously() {
        let mut backend = backend();
        backend.targets.push(target(
            TargetKind::Window,
            12,
            "Zeus Settings",
            Some("com.zeus.zeus"),
        ));
        let request = ScreenshotRequest {
            window: Some("zeu".to_owned()),
            ..Default::default()
        };
        let error = capture_with_backend(&request, &mut backend).expect_err("ambiguous");
        assert_eq!(error.code, "screenshot_target_ambiguous");
        assert_eq!(backend.captures, 0);
    }

    #[test]
    fn multiple_inactive_zeus_windows_fail_closed() {
        let mut backend = backend();
        backend.targets.push(target(
            TargetKind::Window,
            12,
            "Zeus Dev",
            Some("com.zeus.zeus.dev.abc1234"),
        ));
        let error = capture_with_backend(&ScreenshotRequest::default(), &mut backend)
            .expect_err("ambiguous default");
        assert_eq!(error.code, "zeus_window_ambiguous");
        assert_eq!(backend.captures, 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn real_window_soak_is_explicitly_opt_in() {
        if std::env::var("ZEUS_SCREENSHOT_SOAK").ok().as_deref() != Some("1") {
            return;
        }
        let result = capture_in_process(&ScreenshotRequest::default()).expect("real screenshot");
        assert_eq!(result["mimeType"], "image/jpeg");
        assert!(result["data"].as_str().is_some_and(|data| !data.is_empty()));
    }
}
