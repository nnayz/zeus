import Foundation

/// Locates a SwiftPM resource bundle across the places it can actually live —
/// instead of the single spot the generated `Bundle.module` accessor checks and
/// then `fatalError`s on.
///
/// That accessor only tries `Bundle.main.bundleURL/<name>.bundle` (the .app
/// ROOT for a bundled app, where codesign forbids resources) and a hardcoded
/// absolute dev build path. So in a shipped app it crashes on every machine but
/// the author's (whose build path happens to exist). We ship bundles in
/// Contents/Resources and find them here; nil-safe so absent resources degrade
/// (empty catalog) instead of taking the process down.
public enum ResourceBundle {
    /// The `dirijor_DirijorCore` resource bundle (agent manifests), or nil.
    public static let core: Bundle? = find("dirijor_DirijorCore")

    public static func find(_ name: String) -> Bundle? {
        let file = "\(name).bundle"
        let token = Bundle(for: BundleToken.self)
        let candidates: [URL?] = [
            Bundle.main.resourceURL?.appendingPathComponent(file),  // Contents/Resources (shipped app)
            Bundle.main.executableURL?.deletingLastPathComponent()
                .appendingPathComponent(file),                      // next to the executable (swift build / CLI / daemon)
            Bundle.main.bundleURL.appendingPathComponent(file),     // .app root (the stock accessor's spot)
            token.resourceURL?.appendingPathComponent(file),        // this module's own bundle dir (tests / frameworks)
            token.bundleURL.deletingLastPathComponent().appendingPathComponent(file),
        ]
        for case let url? in candidates where FileManager.default.fileExists(atPath: url.path) {
            if let bundle = Bundle(url: url) { return bundle }
        }
        return nil
    }
}

private final class BundleToken {}
