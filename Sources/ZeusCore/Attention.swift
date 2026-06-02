import Foundation

/// What a session deserves from the user, derived from status + the seen bit.
/// Drives sidebar glyphs, project rollups, and the menu-bar icon.
public enum AttentionLevel: Int, Comparable, Codable, Hashable, Sendable {
    case none = 0        // exited & seen, or unknown
    case idleSeen = 1
    case working = 2
    case doneUnseen = 3  // finished a turn and the user hasn't looked yet
    case needsInput = 4

    public static func < (lhs: AttentionLevel, rhs: AttentionLevel) -> Bool {
        lhs.rawValue < rhs.rawValue
    }

    public init(
        status: SessionStatus,
        lastTurnCompletedAt: Date?,
        lastSeenAt: Date?
    ) {
        switch status {
        case .needsInput:
            self = .needsInput
        case .working, .starting:
            self = .working
        case .idle:
            if let completed = lastTurnCompletedAt,
               completed > (lastSeenAt ?? .distantPast) {
                self = .doneUnseen
            } else {
                self = .idleSeen
            }
        case .exited, .unknown:
            self = .none
        }
    }

    /// Rollup for a group of sessions (project row, menu-bar icon): the loudest wins.
    public static func rollup(_ levels: some Sequence<AttentionLevel>) -> AttentionLevel {
        levels.max() ?? .none
    }
}
