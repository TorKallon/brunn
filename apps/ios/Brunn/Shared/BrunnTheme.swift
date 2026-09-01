import SwiftUI

enum AppAppearance: String, CaseIterable, Identifiable {
    case dark
    case light

    static let storageKey = "brunn.appearance.v1"
    static let defaultValue = AppAppearance.dark

    var id: String { rawValue }

    var label: String {
        switch self {
        case .dark: "Dark"
        case .light: "Light"
        }
    }

    var colorScheme: ColorScheme {
        switch self {
        case .dark: .dark
        case .light: .light
        }
    }
}

enum BrunnTheme {
    /// Cobalt beam accent — the single brand hue (docs/Brand.md).
    static let signal = dynamic(light: 0x3158D9, dark: 0x8FA9FF)
    /// Secondary data/info accent.
    static let pulse = dynamic(light: 0x0F7583, dark: 0x5FB9D0)
    static let ink = dynamic(light: 0x1B2130, dark: 0xE7EDF9)
    static let amber = dynamic(light: 0x8B5B09, dark: 0xD9A251)
    static let red = dynamic(light: 0xAC3B47, dark: 0xE08894)
    /// Completion/status ramp. Brand blue must never communicate success.
    static let success = dynamic(light: 0x177A4F, dark: 0x66C695)
    /// Night chrome for brand surfaces (matches LaunchBackground).
    static let night = dynamic(light: 0x06152C, dark: 0x030B18)
    static let canvas = Color(uiColor: .secondarySystemBackground)
    static let line = Color(uiColor: .separator).opacity(0.55)

    // Transitional aliases for the in-flight dashboard feature; migrate its
    // call sites to the canonical `night`, `signal`, and `pulse` once it lands.
    static let navy = night
    static let signalBlue = signal
    static let signalCyan = pulse

    private static func dynamic(light: UInt32, dark: UInt32) -> Color {
        Color(uiColor: UIColor { traits in
            UIColor(rgb: traits.userInterfaceStyle == .dark ? dark : light)
        })
    }
}

private extension UIColor {
    convenience init(rgb: UInt32) {
        self.init(
            red: CGFloat((rgb >> 16) & 0xFF) / 255,
            green: CGFloat((rgb >> 8) & 0xFF) / 255,
            blue: CGFloat(rgb & 0xFF) / 255,
            alpha: 1
        )
    }
}

struct BrandMark: View {
    var size: CGFloat = 32

    var body: some View {
        ZStack {
            LinearGradient(
                colors: [
                    Color(red: 0.012, green: 0.043, blue: 0.094),
                    Color(red: 0.043, green: 0.129, blue: 0.267),
                ],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
            BeamShape()
                .fill(
                    LinearGradient(
                        colors: [
                            .white,
                            Color(red: 0.78, green: 0.85, blue: 1.0),
                            Color(red: 0.19, green: 0.35, blue: 0.85),
                        ],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    )
                )
            Circle()
                .fill(.white)
                .frame(width: size * 0.14, height: size * 0.14)
                .shadow(color: Color(red: 0.56, green: 0.66, blue: 1.0), radius: size * 0.10)
                .position(x: size * 0.32, y: size * 0.30)
        }
        .frame(width: size, height: size)
        .clipShape(RoundedRectangle(cornerRadius: size * 0.28, style: .continuous))
        .accessibilityLabel("Brunn")
    }
}

private struct BeamShape: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        let origin = CGPoint(x: rect.width * 0.32, y: rect.height * 0.30)
        path.move(to: origin)
        path.addLine(to: CGPoint(x: rect.width * 1.05, y: rect.height * 0.78))
        path.addLine(to: CGPoint(x: rect.width * 0.86, y: rect.height * 1.05))
        path.closeSubpath()
        return path
    }
}

struct Eyebrow: View {
    let text: String

    var body: some View {
        Text(text.uppercased())
            .font(.caption.weight(.bold))
            .tracking(0.7)
            .foregroundStyle(BrunnTheme.signal)
    }
}

struct StatusPill: View {
    let text: String
    var color: Color = BrunnTheme.signal
    var symbol: String?

    var body: some View {
        HStack(spacing: 4) {
            if let symbol {
                Image(systemName: symbol)
            }
            Text(text)
        }
        .font(.caption2.weight(.semibold))
        .foregroundStyle(color)
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(color.opacity(0.09), in: RoundedRectangle(cornerRadius: 6))
        .overlay {
            RoundedRectangle(cornerRadius: 6)
                .stroke(color.opacity(0.22), lineWidth: 1)
        }
    }
}

struct SafeMarkdownText: View {
    let markdown: String

    var body: some View {
        Text(SafeMarkdown.attributedString(markdown))
            .tint(BrunnTheme.signal)
    }
}

enum SafeMarkdown {
    static func attributedString(_ markdown: String) -> AttributedString {
        var value = (try? AttributedString(
            markdown: markdown,
            options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        )) ?? AttributedString(markdown)
        for run in value.runs {
            guard let link = run.link else { continue }
            let scheme = link.scheme?.lowercased()
            if scheme != "https", scheme != "http" {
                value[run.range].link = nil
            }
        }
        return value
    }

    static func entryAttributedString(_ markdown: String) -> AttributedString {
        let prepared = EntryMarkdown.rewritingWikiLinks(in: markdown)
        var value = (try? AttributedString(
            markdown: prepared,
            options: .init(interpretedSyntax: .full)
        )) ?? AttributedString(prepared)
        let links = value.runs.compactMap { run in
            run.link.map { (run.range, $0) }
        }
        for (range, linkURL) in links {
            let scheme = linkURL.scheme?.lowercased()
            if scheme == "https" || scheme == "http" {
                continue
            }
            if EntryNavigationURL.link(from: linkURL) != nil {
                continue
            }
            if let entryLink = WorkspaceEntryLink(target: linkURL.relativeString),
               let navigationURL = EntryNavigationURL.url(for: entryLink)
            {
                value[range].link = navigationURL
            } else {
                value[range].link = nil
            }
        }
        return value
    }
}

enum EntryNavigationURL {
    private static let scheme = "brunn-entry"

    static func url(for link: WorkspaceEntryLink) -> URL? {
        var components = URLComponents()
        components.scheme = scheme
        components.host = "open"
        var items = [URLQueryItem(name: "target", value: link.target)]
        if let label = link.label, !label.isEmpty {
            items.append(URLQueryItem(name: "label", value: label))
        }
        if link.isWikiLink {
            items.append(URLQueryItem(name: "wiki", value: "1"))
        }
        components.queryItems = items
        return components.url
    }

    static func link(from url: URL) -> WorkspaceEntryLink? {
        guard url.scheme?.lowercased() == scheme,
              url.host?.lowercased() == "open",
              let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let target = components.queryItems?.first(where: { $0.name == "target" })?.value
        else { return nil }
        let label = components.queryItems?.first(where: { $0.name == "label" })?.value
        let isWiki = components.queryItems?.first(where: { $0.name == "wiki" })?.value == "1"
        return WorkspaceEntryLink(target: target, label: label, isWikiLink: isWiki)
    }
}

enum EntryMarkdown {
    private struct MarkdownLine {
        let text: String
        let allowsInlineRewriting: Bool
    }

    private struct Fence {
        let character: Character
        let count: Int
        let containers: [Container]
    }

    private enum Container {
        case blockquote
        case list(continuationIndent: Int)
    }

    private struct ContainerContent {
        let text: String
        let containers: [Container]
    }

    static func rewritingWikiLinks(in markdown: String) -> String {
        var openFence: Fence?
        let lines = markdown
            .split(separator: "\n", omittingEmptySubsequences: false)
            .map { rawLine in
                let line = String(rawLine)
                if let fence = openFence {
                    if let fencedContent = markdownContent(
                        in: line,
                        matching: fence.containers
                    ) {
                        if let marker = fenceMarker(in: fencedContent),
                       marker.character == fence.character,
                       marker.count >= fence.count,
                       marker.remainder.trimmingCharacters(in: .whitespaces).isEmpty
                        {
                            openFence = nil
                        }
                        return MarkdownLine(text: line, allowsInlineRewriting: false)
                    }
                    openFence = nil
                }

                let containerContent = markdownContainerContent(in: line)
                if let marker = fenceMarker(in: containerContent.text)
                {
                    openFence = Fence(
                        character: marker.character,
                        count: marker.count,
                        containers: containerContent.containers
                    )
                    return MarkdownLine(text: line, allowsInlineRewriting: false)
                }
                if containerContent.text.hasPrefix("    ")
                    || containerContent.text.hasPrefix("\t")
                {
                    return MarkdownLine(text: line, allowsInlineRewriting: false)
                }

                return MarkdownLine(text: line, allowsInlineRewriting: true)
            }

        var inlineCodeTickCount: Int?
        return lines.enumerated()
            .map { index, line in
                guard line.allowsInlineRewriting else {
                    inlineCodeTickCount = nil
                    return line.text
                }
                return rewritingInlineWikiLinks(
                    in: line.text,
                    followingLines: lines.dropFirst(index + 1),
                    codeTickCount: &inlineCodeTickCount
                )
            }
            .joined(separator: "\n")
    }

    private static func rewritingInlineWikiLinks(
        in markdown: String,
        followingLines: ArraySlice<MarkdownLine>,
        codeTickCount: inout Int?
    ) -> String {
        var output = ""
        var cursor = markdown.startIndex

        while cursor < markdown.endIndex {
            let character = markdown[cursor]
            if character == "\\" {
                output.append(character)
                cursor = markdown.index(after: cursor)
                if cursor < markdown.endIndex {
                    output.append(markdown[cursor])
                    cursor = markdown.index(after: cursor)
                }
                continue
            }

            if character == "`" {
                var end = cursor
                var count = 0
                while end < markdown.endIndex, markdown[end] == "`" {
                    count += 1
                    end = markdown.index(after: end)
                }
                output += markdown[cursor ..< end]
                if codeTickCount == nil {
                    if hasClosingCodeDelimiter(
                        count: count,
                        in: markdown[end...],
                        followingLines: followingLines
                    ) {
                        codeTickCount = count
                    }
                } else if codeTickCount == count {
                    codeTickCount = nil
                }
                cursor = end
                continue
            }

            if codeTickCount == nil,
               markdown[cursor...].hasPrefix("[["),
               let openEnd = markdown.index(cursor, offsetBy: 2, limitedBy: markdown.endIndex),
               let close = markdown.range(of: "]]", range: openEnd ..< markdown.endIndex)
            {
                let isEmbed = cursor > markdown.startIndex
                    && markdown[markdown.index(before: cursor)] == "!"
                let raw = String(markdown[openEnd ..< close.lowerBound])
                let pieces = raw.split(
                    separator: "|",
                    maxSplits: 1,
                    omittingEmptySubsequences: false
                )
                let target = pieces.first.map(String.init) ?? ""
                let label = pieces.count > 1
                    ? String(pieces[1]).trimmingCharacters(in: .whitespacesAndNewlines)
                    : defaultWikiLabel(target)

                if !isEmbed,
                   let link = WorkspaceEntryLink(
                       target: target,
                       label: label,
                       isWikiLink: true
                   ),
                   let url = EntryNavigationURL.url(for: link)
                {
                    output += "[\(escapedLabel(label))](<\(url.absoluteString)>)"
                } else {
                    output += markdown[cursor ..< close.upperBound]
                }
                cursor = close.upperBound
                continue
            }

            output.append(character)
            cursor = markdown.index(after: cursor)
        }

        return output
    }

    private static func hasClosingCodeDelimiter(
        count: Int,
        in remainder: Substring,
        followingLines: ArraySlice<MarkdownLine>
    ) -> Bool {
        if containsCodeDelimiter(count: count, in: remainder) {
            return true
        }
        for line in followingLines {
            guard line.allowsInlineRewriting else { return false }
            if containsCodeDelimiter(count: count, in: line.text[...]) {
                return true
            }
        }
        return false
    }

    private static func containsCodeDelimiter(count: Int, in markdown: Substring) -> Bool {
        var cursor = markdown.startIndex
        while cursor < markdown.endIndex {
            if markdown[cursor] == "\\" {
                cursor = markdown.index(after: cursor)
                if cursor < markdown.endIndex {
                    cursor = markdown.index(after: cursor)
                }
                continue
            }
            guard markdown[cursor] == "`" else {
                cursor = markdown.index(after: cursor)
                continue
            }

            var end = cursor
            var runCount = 0
            while end < markdown.endIndex, markdown[end] == "`" {
                runCount += 1
                end = markdown.index(after: end)
            }
            if runCount == count {
                return true
            }
            cursor = end
        }
        return false
    }

    private static func markdownContainerContent(in line: String) -> ContainerContent {
        var contentStart = line.startIndex
        var containers: [Container] = []

        while contentStart < line.endIndex {
            var marker = contentStart
            var spaces = 0
            while marker < line.endIndex, line[marker] == " ", spaces < 3 {
                spaces += 1
                marker = line.index(after: marker)
            }

            if marker < line.endIndex, line[marker] == ">" {
                marker = line.index(after: marker)
                if marker < line.endIndex,
                   line[marker] == " " || line[marker] == "\t"
                {
                    marker = line.index(after: marker)
                }
                containers.append(.blockquote)
                contentStart = marker
                continue
            }

            guard let listMarker = listMarker(
                in: line,
                at: marker,
                column: containers.reduce(0) { width, container in
                    switch container {
                    case .blockquote:
                        return width
                    case let .list(continuationIndent):
                        return width + continuationIndent
                    }
                } + spaces
            ) else { break }
            containers.append(.list(continuationIndent: spaces + listMarker.width))
            contentStart = listMarker.contentStart
        }
        return ContainerContent(
            text: String(line[contentStart...]),
            containers: containers
        )
    }

    private static func markdownContent(
        in line: String,
        matching containers: [Container]
    ) -> String? {
        var contentStart = line.startIndex

        for container in containers {
            if line[contentStart...].trimmingCharacters(in: .whitespaces).isEmpty {
                return ""
            }

            switch container {
            case .blockquote:
                var marker = contentStart
                var spaces = 0
                while marker < line.endIndex, line[marker] == " ", spaces < 3 {
                    spaces += 1
                    marker = line.index(after: marker)
                }
                guard marker < line.endIndex, line[marker] == ">" else { return nil }
                marker = line.index(after: marker)
                if marker < line.endIndex,
                   line[marker] == " " || line[marker] == "\t"
                {
                    marker = line.index(after: marker)
                }
                contentStart = marker

            case let .list(continuationIndent):
                guard let remainder = droppingIndent(
                    continuationIndent,
                    from: String(line[contentStart...])
                ) else { return nil }
                contentStart = line.index(
                    line.endIndex,
                    offsetBy: -remainder.count
                )
            }
        }

        return String(line[contentStart...])
    }

    private static func listMarker(
        in line: String,
        at start: String.Index,
        column: Int
    ) -> (contentStart: String.Index, width: Int)? {
        guard start < line.endIndex else { return nil }
        var markerEnd = start

        if line[markerEnd] == "-" || line[markerEnd] == "+" || line[markerEnd] == "*" {
            markerEnd = line.index(after: markerEnd)
        } else {
            var digitCount = 0
            while markerEnd < line.endIndex,
                  line[markerEnd].isNumber,
                  digitCount < 9
            {
                digitCount += 1
                markerEnd = line.index(after: markerEnd)
            }
            guard digitCount > 0,
                  markerEnd < line.endIndex,
                  line[markerEnd] == "." || line[markerEnd] == ")"
            else { return nil }
            markerEnd = line.index(after: markerEnd)
        }

        guard markerEnd < line.endIndex,
              line[markerEnd] == " " || line[markerEnd] == "\t"
        else { return nil }

        let markerWidth = line.distance(from: start, to: markerEnd)
        if line[markerEnd] == "\t" {
            let tabWidth = 4 - ((column + markerWidth) % 4)
            return (line.index(after: markerEnd), markerWidth + tabWidth)
        }

        var contentStart = markerEnd
        var whitespaceCount = 0
        while contentStart < line.endIndex,
              line[contentStart] == " ",
              whitespaceCount < 4
        {
            whitespaceCount += 1
            contentStart = line.index(after: contentStart)
        }
        return (contentStart, markerWidth + whitespaceCount)
    }

    private static func droppingIndent(_ width: Int, from line: String) -> String? {
        var index = line.startIndex
        var consumed = 0
        while index < line.endIndex, consumed < width {
            if line[index] == " " {
                consumed += 1
            } else if line[index] == "\t" {
                consumed += 4
            } else {
                return nil
            }
            index = line.index(after: index)
        }
        guard consumed >= width else { return nil }
        return String(line[index...])
    }

    private static func fenceMarker(
        in line: String
    ) -> (character: Character, count: Int, remainder: String)? {
        var index = line.startIndex
        var spaces = 0
        while index < line.endIndex, line[index] == " ", spaces < 4 {
            spaces += 1
            index = line.index(after: index)
        }
        guard spaces <= 3, index < line.endIndex else { return nil }
        let character = line[index]
        guard character == "`" || character == "~" else { return nil }

        var end = index
        var count = 0
        while end < line.endIndex, line[end] == character {
            count += 1
            end = line.index(after: end)
        }
        guard count >= 3 else { return nil }
        return (character, count, String(line[end...]))
    }

    private static func defaultWikiLabel(_ raw: String) -> String {
        let withoutHeading = raw.split(separator: "#", maxSplits: 1).first.map(String.init) ?? raw
        let withoutBlock = withoutHeading.split(separator: "^", maxSplits: 1).first.map(String.init)
            ?? withoutHeading
        let name = withoutBlock.split(separator: "/").last.map(String.init) ?? withoutBlock
        if name.lowercased().hasSuffix(".markdown") {
            return String(name.dropLast(9))
        }
        if name.lowercased().hasSuffix(".md") {
            return String(name.dropLast(3))
        }
        return name
    }

    private static func escapedLabel(_ label: String) -> String {
        label
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "[", with: "\\[")
            .replacingOccurrences(of: "]", with: "\\]")
    }
}

struct BoundaryNotice: View {
    let symbol: String
    let title: String
    let detail: String

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: symbol)
                .font(.system(size: 30, weight: .medium))
                .foregroundStyle(BrunnTheme.signal)
            Text(title)
                .font(.headline)
                .multilineTextAlignment(.center)
            Text(detail)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(24)
        .background(.background, in: RoundedRectangle(cornerRadius: 8))
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .stroke(BrunnTheme.line, lineWidth: 1)
        }
    }
}

enum DisplayDate {
    static func day(_ dateString: String) -> String {
        guard let date = parse(dateString) else { return dateString }
        return date.formatted(.dateTime.weekday(.wide).month(.wide).day())
    }

    static func time(
        _ dateString: String?,
        timeZoneIdentifier: String? = nil,
        includeZone: Bool = false
    ) -> String? {
        guard let dateString, let date = parse(dateString) else { return nil }
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateStyle = .none
        formatter.timeStyle = .short
        let timeZone = timeZoneIdentifier.flatMap(TimeZone.init(identifier:))
        if let timeZone { formatter.timeZone = timeZone }
        let rendered = formatter.string(from: date)
        guard includeZone, let timeZone else { return rendered }
        let zone = timeZone.abbreviation(for: date) ?? timeZone.identifier
        return "\(rendered) \(zone)"
    }

    static func relative(_ date: Date) -> String {
        date.formatted(.relative(presentation: .named, unitsStyle: .wide))
    }

    static func metadata(_ value: String) -> String {
        if value.range(of: #"^\d{4}-\d{2}-\d{2}$"#, options: .regularExpression) != nil {
            let parts = value.split(separator: "-").compactMap { Int($0) }
            if parts.count == 3,
               let date = Calendar(identifier: .gregorian).date(
                   from: DateComponents(year: parts[0], month: parts[1], day: parts[2])
               )
            {
                return date.formatted(date: .abbreviated, time: .omitted)
            }
            return value
        }
        guard let date = parse(value) else { return value }
        return date.formatted(date: .abbreviated, time: .shortened)
    }

    private static func parse(_ value: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: value) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        if let date = formatter.date(from: value) { return date }

        let dayFormatter = DateFormatter()
        dayFormatter.locale = Locale(identifier: "en_US_POSIX")
        dayFormatter.dateFormat = "yyyy-MM-dd"
        return dayFormatter.date(from: value)
    }
}
