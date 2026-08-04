import Foundation

/// Region extraction from a `ScreenSnapshot`. All functions are pure and cheap;
/// they operate on the plain-text grid lines the daemon's emulator produced.
enum Regions {
    /// Box-drawing / border glyphs used to detect and strip prompt boxes.
    static let borderChars: Set<Character> = [
        "╭", "╮", "╰", "╯", "│", "─",
        "┌", "┐", "└", "┘", "┃", "┏", "┓", "┗", "┛", "━",
        "┤", "├", "┬", "┴", "┼",
    ]

    /// Prompt markers that can begin a (possibly unboxed) prompt line.
    static let promptStartChars: Set<Character> = ["╭", "│", "╰", "❯", "›", "»"]

    private static func isBlank(_ line: String) -> Bool {
        line.trimmingCharacters(in: .whitespaces).isEmpty
    }

    /// Non-blank lines, order preserved.
    static func nonBlank(_ lines: [String]) -> [String] {
        lines.filter { !isBlank($0) }
    }

    /// `whole_recent`: the last 60 non-blank lines.
    static func wholeRecent(_ lines: [String]) -> [String] {
        Array(nonBlank(lines).suffix(60))
    }

    /// `bottom_non_empty_lines(n)`: the last `n` non-blank lines.
    static func bottomNonEmpty(_ lines: [String], _ n: Int) -> [String] {
        guard n > 0 else { return [] }
        return Array(nonBlank(lines).suffix(n))
    }

    private static func isBoxLine(_ line: String) -> Bool {
        line.contains { borderChars.contains($0) }
    }

    /// Strips leading/trailing border glyphs (and surrounding whitespace) from a
    /// single box line, returning the interior text — or `nil` when the line is a
    /// pure border/separator (nothing meaningful inside).
    static func stripBox(_ line: String) -> String? {
        var t = Substring(line.trimmingCharacters(in: .whitespaces))
        while let f = t.first, borderChars.contains(f) { t = t.dropFirst() }
        while let l = t.last, borderChars.contains(l) { t = t.dropLast() }
        if t.trimmingCharacters(in: .whitespaces).isEmpty { return nil }
        return String(t)
    }

    /// `prompt_box_body`: the interior text of the bottom-most bordered box, or —
    /// absent a box — the tail starting at the last prompt-marker line.
    static func promptBoxBody(_ lines: [String]) -> [String] {
        // Prefer the last bordered box (a run of lines containing a vertical bar).
        if let lastBar = lines.indices.last(where: { lines[$0].contains("│") }) {
            var start = lastBar
            var end = lastBar
            while start - 1 >= 0, isBoxLine(lines[start - 1]) { start -= 1 }
            while end + 1 < lines.count, isBoxLine(lines[end + 1]) { end += 1 }
            return lines[start...end].compactMap(stripBox)
        }
        // Fallback: last line that starts (after whitespace) with a prompt marker.
        if let idx = lines.indices.last(where: { i in
            let t = lines[i].trimmingCharacters(in: .whitespaces)
            return t.first.map { promptStartChars.contains($0) } ?? false
        }) {
            return nonBlank(Array(lines[idx...]))
        }
        return []
    }

    private static let optionRegex = try! NSRegularExpression(
        pattern: #"^\s*[❯>›»]?\s*(\d+)[.)]\s+(.+)$"#)

    /// Extracts numbered options (`1. Yes`, `❯ 2. No …`) from blocker lines,
    /// returning the option label text.
    static func numberedOptions(_ lines: [String]) -> [String] {
        var out: [String] = []
        for line in lines {
            let range = NSRange(line.startIndex..., in: line)
            guard let m = optionRegex.firstMatch(in: line, options: [], range: range),
                  m.numberOfRanges >= 3,
                  let r = Range(m.range(at: 2), in: line)
            else { continue }
            out.append(String(line[r]).trimmingCharacters(in: .whitespaces))
        }
        return out
    }

    /// Extracts the raw lines for `region` from `snapshot`.
    static func extract(_ region: RegionKind, regionLines n: Int, from snapshot: ScreenSnapshot) -> [String] {
        switch region {
        case .osc_title:
            return snapshot.oscTitle.map { [$0] } ?? []
        case .osc_progress:
            // The `progress` predicate reads oscProgressState directly; the region
            // itself carries no text.
            return []
        case .whole_recent:
            return wholeRecent(snapshot.lines)
        case .bottom_non_empty_lines:
            return bottomNonEmpty(snapshot.lines, n)
        case .prompt_box_body:
            return promptBoxBody(snapshot.lines)
        }
    }
}
