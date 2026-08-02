import SwiftUI

enum StraylightTheme {
    static let forest = Color(red: 0.055, green: 0.384, blue: 0.286)
    static let ink = Color(red: 0.09, green: 0.10, blue: 0.10)
    static let canvas = Color(uiColor: .secondarySystemBackground)
    static let line = Color(uiColor: .separator).opacity(0.55)
    static let blue = Color(red: 0.11, green: 0.42, blue: 0.62)
    static let amber = Color(red: 0.66, green: 0.40, blue: 0.10)
    static let red = Color(red: 0.68, green: 0.16, blue: 0.16)
}

struct BrandMark: View {
    var body: some View {
        Text("S")
            .font(.system(.headline, design: .rounded, weight: .bold))
            .foregroundStyle(.white)
            .frame(width: 32, height: 32)
            .background(StraylightTheme.forest, in: RoundedRectangle(cornerRadius: 7))
            .accessibilityLabel("Straylight")
    }
}

struct Eyebrow: View {
    let text: String

    var body: some View {
        Text(text.uppercased())
            .font(.caption.weight(.bold))
            .tracking(0.7)
            .foregroundStyle(StraylightTheme.forest)
    }
}

struct StatusPill: View {
    let text: String
    var color: Color = StraylightTheme.forest
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
            .tint(StraylightTheme.forest)
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
}

struct BoundaryNotice: View {
    let symbol: String
    let title: String
    let detail: String

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: symbol)
                .font(.system(size: 30, weight: .medium))
                .foregroundStyle(StraylightTheme.forest)
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
                .stroke(StraylightTheme.line, lineWidth: 1)
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
