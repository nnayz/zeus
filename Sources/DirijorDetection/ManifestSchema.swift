import DirijorCore
import Foundation

// MARK: - Sendable regex wrapper

/// `NSRegularExpression` is documented immutable and thread-safe, but the Swift
/// checker doesn't know that. This wrapper compiles once at manifest load and is
/// safe to share across the daemon's session actors.
struct SendableRegex: @unchecked Sendable {
    let pattern: String
    private let regex: NSRegularExpression

    init(pattern: String) throws {
        self.pattern = pattern
        self.regex = try NSRegularExpression(pattern: pattern, options: [])
    }

    func matches(_ text: String) -> Bool {
        let range = NSRange(text.startIndex..., in: text)
        return regex.firstMatch(in: text, options: [], range: range) != nil
    }
}

// MARK: - Region kinds

enum RegionKind: String, Sendable, Decodable {
    case osc_title
    case osc_progress
    case whole_recent
    case prompt_box_body
    case bottom_non_empty_lines
}

// MARK: - Predicate

/// The evaluation context handed to a predicate: the region's joined text, its
/// individual lines, and the snapshot's OSC progress state (used by `progress`).
struct PredicateContext {
    let text: String
    let lines: [String]
    let progressState: Int?
}

/// Recursive detection predicate. Regexes are compiled once at load and cached.
enum Predicate: Sendable {
    case contains(String)          // case-insensitive substring over region text
    case regex(SendableRegex)      // matches the whole region text
    case lineRegex(SendableRegex)  // matches if ANY region line matches
    case progress(state: Int)      // matches oscProgressState
    indirect case any([Predicate])
    indirect case all([Predicate])
    indirect case not(Predicate)

    func evaluate(_ ctx: PredicateContext) -> Bool {
        switch self {
        case .contains(let needle):
            return ctx.text.range(of: needle, options: .caseInsensitive) != nil
        case .regex(let re):
            return re.matches(ctx.text)
        case .lineRegex(let re):
            return ctx.lines.contains { re.matches($0) }
        case .progress(let state):
            return ctx.progressState == state
        case .any(let ps):
            return ps.contains { $0.evaluate(ctx) }
        case .all(let ps):
            return ps.allSatisfy { $0.evaluate(ctx) }
        case .not(let p):
            return !p.evaluate(ctx)
        }
    }
}

extension Predicate: Decodable {
    private enum CodingKeys: String, CodingKey {
        case contains, regex, lineRegex, progress, any, all, not
    }

    private struct ProgressSpec: Decodable { let state: Int }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        if let s = try c.decodeIfPresent(String.self, forKey: .contains) {
            self = .contains(s)
        } else if let s = try c.decodeIfPresent(String.self, forKey: .regex) {
            self = .regex(try SendableRegex(pattern: s))
        } else if let s = try c.decodeIfPresent(String.self, forKey: .lineRegex) {
            self = .lineRegex(try SendableRegex(pattern: s))
        } else if let p = try c.decodeIfPresent(ProgressSpec.self, forKey: .progress) {
            self = .progress(state: p.state)
        } else if let a = try c.decodeIfPresent([Predicate].self, forKey: .any) {
            self = .any(a)
        } else if let a = try c.decodeIfPresent([Predicate].self, forKey: .all) {
            self = .all(a)
        } else if let n = try c.decodeIfPresent(Predicate.self, forKey: .not) {
            self = .not(n)
        } else {
            throw DecodingError.dataCorrupted(
                .init(codingPath: decoder.codingPath,
                      debugDescription: "Predicate object had no recognized key"))
        }
    }
}

// MARK: - Capture

struct Capture: Sendable, Decodable {
    let region: RegionKind
    let regionLines: Int
    let maxChars: Int

    private enum CodingKeys: String, CodingKey { case region, regionLines, maxChars }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        region = try c.decode(RegionKind.self, forKey: .region)
        regionLines = try c.decodeIfPresent(Int.self, forKey: .regionLines) ?? 5
        maxChars = try c.decodeIfPresent(Int.self, forKey: .maxChars) ?? 400
    }
}

// MARK: - Rule

struct Rule: Sendable, Decodable {
    let id: String
    let state: ManifestState
    let priority: Int
    let region: RegionKind
    let regionLines: Int
    let when: Predicate
    let flags: Set<String>
    let capture: Capture?

    private enum CodingKeys: String, CodingKey {
        case id, state, priority, region, regionLines, when, flags, capture
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        state = try c.decode(ManifestState.self, forKey: .state)
        priority = try c.decode(Int.self, forKey: .priority)
        region = try c.decode(RegionKind.self, forKey: .region)
        regionLines = try c.decodeIfPresent(Int.self, forKey: .regionLines) ?? 5
        when = try c.decode(Predicate.self, forKey: .when)
        flags = Set(try c.decodeIfPresent([String].self, forKey: .flags) ?? [])
        capture = try c.decodeIfPresent(Capture.self, forKey: .capture)
    }

    var isBlocker: Bool { state == .blockedPermission || state == .blockedQuestion }
}

// MARK: - Manifest

enum StatusModel: String, Sendable, Decodable {
    case full
    case processOnly
}

struct Manifest: Sendable, Decodable {
    let schemaVersion: Int
    let id: String
    let version: String
    let statusModel: StatusModel
    /// Pre-sorted by descending priority (ties keep manifest order) so the
    /// evaluator stops at the first matching rule — which is then guaranteed to
    /// be the highest-priority match — instead of scoring all ~10 rules every
    /// frame. Sorting once at load keeps this off the 5Hz-per-session hot path.
    let rules: [Rule]

    private enum CodingKeys: String, CodingKey {
        case schemaVersion, id, version, statusModel, rules
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try c.decode(Int.self, forKey: .schemaVersion)
        id = try c.decode(String.self, forKey: .id)
        version = try c.decode(String.self, forKey: .version)
        statusModel = try c.decode(StatusModel.self, forKey: .statusModel)
        let decoded = try c.decode([Rule].self, forKey: .rules)
        rules = decoded.enumerated()
            .sorted {
                $0.element.priority != $1.element.priority
                    ? $0.element.priority > $1.element.priority
                    : $0.offset < $1.offset
            }
            .map(\.element)
    }
}

extension ManifestState {
    var needsInputKind: NeedsInputKind? {
        switch self {
        case .blockedPermission: return .permission
        case .blockedQuestion: return .question
        default: return nil
        }
    }
}
