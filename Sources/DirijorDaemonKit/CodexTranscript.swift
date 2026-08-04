import Foundation

/// Bounded readers for Codex's append-only rollout transcripts.
enum CodexTranscript {
    private static let firstPromptCap = 8 << 20

    /// Locate a rollout from its thread id without walking the entire Codex
    /// history. UUIDs and Dirijor records are created together, so the UTC day
    /// of `createdAt` (plus a one-day boundary cushion) contains the file.
    static func find(
        agentID: String,
        createdAt: Date,
        sessionsRoot: URL,
        fileManager: FileManager = .default
    ) -> URL? {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(secondsFromGMT: 0)!

        for dayOffset in [0, -1, 1] {
            guard let date = calendar.date(byAdding: .day, value: dayOffset, to: createdAt) else {
                continue
            }
            let parts = calendar.dateComponents([.year, .month, .day], from: date)
            guard let year = parts.year, let month = parts.month, let day = parts.day else {
                continue
            }
            let directory = sessionsRoot
                .appendingPathComponent(String(format: "%04d", year))
                .appendingPathComponent(String(format: "%02d", month))
                .appendingPathComponent(String(format: "%02d", day))
            guard let files = try? fileManager.contentsOfDirectory(
                at: directory, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
            else { continue }
            if let match = files.first(where: {
                $0.lastPathComponent.hasSuffix("-\(agentID).jsonl")
            }) {
                return match
            }
        }
        return nil
    }

    /// The first real user prompt from an `event_msg/user_message` record.
    /// Stops as soon as it is found and never reads more than 8 MiB.
    static func firstUserPrompt(file: URL) -> String? {
        guard let handle = try? FileHandle(forReadingFrom: file) else { return nil }
        defer { try? handle.close() }

        var buffered = Data()
        var total = 0
        while total < firstPromptCap {
            let want = min(64 << 10, firstPromptCap - total)
            guard let chunk = try? handle.read(upToCount: want), !chunk.isEmpty else { break }
            buffered.append(chunk)
            total += chunk.count

            while let newline = buffered.firstIndex(of: 0x0A) {
                let line = buffered[..<newline]
                buffered.removeSubrange(...newline)
                if let prompt = userPrompt(from: line) { return prompt }
            }
        }
        return buffered.isEmpty ? nil : userPrompt(from: buffered[...])
    }

    private static func userPrompt(from line: Data.SubSequence) -> String? {
        guard line.range(of: Data(#""user_message""#.utf8)) != nil,
            let obj = try? JSONSerialization.jsonObject(with: Data(line)) as? [String: Any],
            obj["type"] as? String == "event_msg",
            let payload = obj["payload"] as? [String: Any],
            payload["type"] as? String == "user_message",
            let message = payload["message"] as? String,
            !message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return nil }
        return message
    }
}
