import SwiftUI

enum StraylightTheme {
    /// Cobalt beam accent — the single brand hue (docs/Brand.md).
    static let signal = dynamic(light: 0x3158D9, dark: 0x8FA9FF)
    /// Secondary data/info accent.
    static let pulse = dynamic(light: 0x0F7583, dark: 0x5FB9D0)
    static let ink = dynamic(light: 0x1B2130, dark: 0xE7EDF9)
    static let amber = dynamic(light: 0x8B5B09, dark: 0xD9A251)
    static let red = dynamic(light: 0xAC3B47, dark: 0xE08894)
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
        .accessibilityLabel("Straylight")
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
            .foregroundStyle(StraylightTheme.signal)
    }
}

struct StatusPill: View {
    let text: String
    var color: Color = StraylightTheme.signal
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
            .tint(StraylightTheme.signal)
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
                .foregroundStyle(StraylightTheme.signal)
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
