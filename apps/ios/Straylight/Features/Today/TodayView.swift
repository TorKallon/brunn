import SwiftUI

struct TodayView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if let message = model.connectionMessage, !model.isDemo {
                        ConnectionBanner(message: message, isDemo: model.isDemo)
                    }

                    if let briefing = model.latestBriefing {
                        BriefingReader(
                            briefing: briefing,
                            cachedAt: model.cachedAt,
                            focusedItemID: model.focusedBriefingItemID
                        )
                        .id(briefing.entryRef)
                    } else {
                        BoundaryNotice(
                            symbol: "sunrise",
                            title: "No briefing is published yet",
                            detail: "When an agent publishes a structured briefing, it will appear here."
                        )
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 12)
                .padding(.top, 16)
                .padding(.bottom, 32)
            }
            .background(StraylightTheme.canvas)
            .navigationTitle("Today")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    BrandMark()
                }
            }
            .refreshable {
                await model.refreshBriefing()
            }
            .overlay {
                if model.isRefreshingBriefing {
                    ProgressView()
                        .padding(12)
                        .background(.regularMaterial, in: Circle())
                        .accessibilityLabel("Refreshing briefing")
                }
            }
            .onAppear {
                scrollToFocusedItem(using: proxy)
            }
            .onChange(of: model.focusedBriefingItemID) { _, _ in
                scrollToFocusedItem(using: proxy)
            }
        }
    }

    private func scrollToFocusedItem(using proxy: ScrollViewProxy) {
        guard let itemID = model.focusedBriefingItemID else { return }
        Task { @MainActor in
            await Task.yield()
            if reduceMotion {
                proxy.scrollTo(itemID, anchor: .top)
            } else {
                withAnimation(.easeInOut(duration: 0.2)) {
                    proxy.scrollTo(itemID, anchor: .top)
                }
            }
        }
    }
}

private struct ConnectionBanner: View {
    let message: String
    let isDemo: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: isDemo ? "sparkles" : "wifi.exclamationmark")
                .foregroundStyle(isDemo ? StraylightTheme.pulse : StraylightTheme.amber)
                .accessibilityHidden(true)
            Text(message)
                .font(.footnote)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(11)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background, in: RoundedRectangle(cornerRadius: 6))
        .overlay {
            RoundedRectangle(cornerRadius: 6)
                .stroke(StraylightTheme.line, lineWidth: 1)
        }
    }
}

struct BriefingReader: View {
    let briefing: BriefingEditionData
    let cachedAt: Date?
    let focusedItemID: String?

    @State private var showAllSummary = true
    @State private var expandedItems: Set<String> = []
    @State private var showsHistory = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            BriefingReaderHeader(briefing: briefing, cachedAt: cachedAt)

            if let payload = briefing.briefing {
                if let summary = payload.summaryMD, !summary.isEmpty {
                    BriefingSummary(
                        lines: summary,
                        showAll: $showAllSummary
                    )
                }

                ForEach(payload.sections ?? []) { section in
                    BriefingSectionView(
                        section: section,
                        expandedItems: $expandedItems,
                        toggle: toggle
                    )
                }
            } else {
                LegacyBriefing(markdown: briefing.markdown)
            }

            if !briefing.versions.isEmpty {
                BriefingRevisionHistory(
                    briefing: briefing,
                    isExpanded: $showsHistory
                )
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("briefing-reader")
        .onAppear {
            if let focusedItemID {
                expandedItems.insert(focusedItemID)
            }
        }
        .onChange(of: focusedItemID) { _, newValue in
            if let newValue {
                expandedItems.insert(newValue)
            }
        }
    }

    private func toggle(_ id: String) {
        let update = {
            if expandedItems.contains(id) {
                expandedItems.remove(id)
            } else {
                expandedItems.insert(id)
            }
        }
        if reduceMotion {
            update()
        } else {
            withAnimation(.easeInOut(duration: 0.18), update)
        }
    }
}

private struct BriefingReaderHeader: View {
    let briefing: BriefingEditionData
    let cachedAt: Date?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.title2.bold())
                .foregroundStyle(StraylightTheme.ink)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityIdentifier("briefing-reader-title")

            Text(publicationLine)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            if let cachedAt {
                Label(
                    "Protected offline copy saved \(DisplayDate.relative(cachedAt)). Pull to retry.",
                    systemImage: "lock.fill"
                )
                .font(.caption)
                .foregroundStyle(StraylightTheme.amber)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var title: String {
        "\(briefing.edition.replacingOccurrences(of: "_", with: " ").localizedCapitalized) briefing - \(briefing.date)"
    }

    private var publicationLine: String {
        let timeZone = briefing.briefing?.timezone
        let generated = BriefingReaderDate.dateTime(
            briefing.briefing?.generatedAt ?? briefing.createdAt,
            timeZoneIdentifier: timeZone
        ) ?? "Unknown"
        var text = "Generated \(generated)"
        if briefing.currentVersion > 1,
           let latest = briefing.versions.last,
           let updated = BriefingReaderDate.dateTime(
               latest.createdAt,
               timeZoneIdentifier: timeZone
           )
        {
            text += " · Updated \(updated)"
        }
        return text
    }
}

private struct BriefingSummary: View {
    let lines: [String]
    @Binding var showAll: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("30-SECOND SUMMARY")
                .font(.caption.weight(.bold))
                .foregroundStyle(.secondary)

            VStack(alignment: .leading, spacing: 10) {
                ForEach(Array(visibleLines.enumerated()), id: \.offset) { index, line in
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        Circle()
                            .fill(StraylightTheme.ink)
                            .frame(width: 5, height: 5)
                            .accessibilityHidden(true)
                        SafeMarkdownText(markdown: line)
                            .font(.body)
                            .foregroundStyle(StraylightTheme.ink)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .fixedSize(horizontal: false, vertical: true)
                            .textSelection(.enabled)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("briefing-summary-line-\(index)")
                }
            }

            if lines.count > previewCount {
                Button(showAll ? "Show less" : "\(lines.count - previewCount) more") {
                    let update = { showAll.toggle() }
                    if reduceMotion {
                        update()
                    } else {
                        withAnimation(.easeInOut(duration: 0.18), update)
                    }
                }
                .buttonStyle(.plain)
                .font(.body.weight(.semibold))
                .foregroundStyle(StraylightTheme.ink)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .frame(minHeight: 44)
                .background(.background, in: RoundedRectangle(cornerRadius: 6))
                .overlay {
                    RoundedRectangle(cornerRadius: 6)
                        .stroke(Color(uiColor: .separator), lineWidth: 1)
                }
                .accessibilityLabel(
                    showAll
                        ? "Show fewer summary items"
                        : "Show all \(lines.count) summary items"
                )
                .accessibilityIdentifier("briefing-summary-toggle")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 13)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background, in: RoundedRectangle(cornerRadius: 6))
        .overlay {
            RoundedRectangle(cornerRadius: 6)
                .stroke(StraylightTheme.line, lineWidth: 1)
        }
        .overlay(alignment: .leading) {
            UnevenRoundedRectangle(
                topLeadingRadius: 6,
                bottomLeadingRadius: 6
            )
            .fill(StraylightTheme.signal)
            .frame(width: 3)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("briefing-summary")
    }

    private let previewCount = 3

    private var visibleLines: [String] {
        showAll ? lines : Array(lines.prefix(previewCount))
    }
}

private struct BriefingSectionView: View {
    let section: BriefingSection
    @Binding var expandedItems: Set<String>
    let toggle: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: 9) {
                Text(section.title)
                    .font(.headline)
                    .foregroundStyle(StraylightTheme.ink)
                Text("\(section.items.count) \(section.items.count == 1 ? "item" : "items")")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)

            Divider()

            ForEach(section.items) { item in
                BriefingItemDisclosure(
                    item: item,
                    sectionTitle: section.title,
                    isExpanded: expandedItems.contains(item.id),
                    onToggle: { toggle(item.id) }
                )
                .id(item.id)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(Color(uiColor: .separator))
                .frame(height: 1)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("briefing-section-\(section.topic)")
    }
}

private struct BriefingItemDisclosure: View {
    let item: BriefingItem
    let sectionTitle: String
    let isExpanded: Bool
    let onToggle: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button(action: onToggle) {
                VStack(alignment: .leading, spacing: 5) {
                    Text(sectionTitle.uppercased())
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)

                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        SafeMarkdownText(markdown: item.headlineMD)
                            .font(.headline)
                            .foregroundStyle(StraylightTheme.ink)
                            .multilineTextAlignment(.leading)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .fixedSize(horizontal: false, vertical: true)

                        if let delta = item.delta {
                            DeltaPill(delta: delta)
                        }

                        Image(systemName: "chevron.down")
                            .font(.caption.weight(.bold))
                            .foregroundStyle(.secondary)
                            .rotationEffect(.degrees(isExpanded ? 180 : 0))
                            .accessibilityHidden(true)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .padding(.horizontal, 12)
            .padding(.vertical, 11)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .accessibilityLabel("\(sectionTitle). \(plainHeadline). \(isExpanded ? "Expanded" : "Collapsed")")
            .accessibilityIdentifier("briefing-item-\(item.id)")

            if isExpanded {
                BriefingItemDetail(item: item)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 12)
                    .transition(.opacity.combined(with: .move(edge: .top)))
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("briefing-item-detail-\(item.id)")
            }

            Divider()
        }
    }

    private var plainHeadline: String {
        String(SafeMarkdown.attributedString(item.headlineMD).characters)
    }
}

private struct DeltaPill: View {
    let delta: String

    var body: some View {
        Text(label)
            .font(.caption2.weight(.bold))
            .foregroundStyle(tint)
            .padding(.horizontal, 7)
            .padding(.vertical, 4)
            .background(tint.opacity(0.09), in: RoundedRectangle(cornerRadius: 5))
            .overlay {
                RoundedRectangle(cornerRadius: 5)
                    .stroke(tint.opacity(0.32), lineWidth: 1)
            }
            .fixedSize()
    }

    private var label: String {
        switch delta {
        case "update": "UPDATE"
        case "corroboration": "SEEN"
        case "correction": "CORRECTION"
        default: "NEW"
        }
    }

    private var tint: Color {
        switch delta {
        case "update": StraylightTheme.pulse
        case "corroboration": StraylightTheme.amber
        case "correction": StraylightTheme.red
        default: StraylightTheme.signal
        }
    }
}

private struct BriefingItemDetail: View {
    let item: BriefingItem

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let body = item.bodyMD, !body.isEmpty {
                SafeMarkdownText(markdown: body)
                    .font(.body)
                    .foregroundStyle(StraylightTheme.ink)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }

            if let detail = item.detailMD, !detail.isEmpty {
                SafeMarkdownText(markdown: detail)
                    .font(.body)
                    .foregroundStyle(StraylightTheme.ink)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }

            if let changed = item.whatChanged, !changed.isEmpty {
                LabeledDetail(title: "What changed", text: changed)
            }

            if let why = item.whyItMatters, !why.isEmpty {
                LabeledDetail(title: "Why it matters", text: why)
            }

            ItemProvenance(item: item)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 6))
        .overlay {
            RoundedRectangle(cornerRadius: 6)
                .stroke(StraylightTheme.line, lineWidth: 1)
        }
    }
}

private struct LabeledDetail: View {
    let title: String
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title)
                .font(.subheadline.italic())
                .foregroundStyle(.secondary)
            Text(text)
                .font(.body)
                .foregroundStyle(StraylightTheme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
        }
    }
}

private struct ItemProvenance: View {
    let item: BriefingItem

    private var safeURLs: [URL] {
        (item.story?.urls ?? []).compactMap(URL.init(string:)).filter {
            $0.scheme?.lowercased() == "https" || $0.scheme?.lowercased() == "http"
        }
    }

    private var timestamps: [String] {
        let values: [(String, String?)] = [
            ("Published", item.times?.publishedAt),
            ("Event", item.times?.eventAt ?? item.story?.eventAt),
            ("First seen", item.times?.firstSeenAt),
        ]
        return values.compactMap { label, value in
            guard let value, !value.isEmpty else { return nil }
            return "\(label) \(BriefingReaderDate.compact(value) ?? value)"
        }
    }

    var body: some View {
        if !safeURLs.isEmpty || !timestamps.isEmpty {
            VStack(alignment: .leading, spacing: 7) {
                if !safeURLs.isEmpty {
                    Text("SOURCES")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(.secondary)

                    ForEach(safeURLs, id: \.absoluteString) { url in
                        Link(destination: url) {
                            Label(sourceLabel(url), systemImage: "arrow.up.right.square")
                                .font(.subheadline.weight(.semibold))
                                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                        }
                    }
                }

                if !timestamps.isEmpty {
                    Text(timestamps.joined(separator: " · "))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private func sourceLabel(_ url: URL) -> String {
        (url.host ?? "Open source").replacingOccurrences(of: "www.", with: "")
    }
}

private struct LegacyBriefing: View {
    let markdown: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Briefing")
                    .font(.headline)
                Spacer()
                StatusPill(text: "Legacy Markdown", color: StraylightTheme.amber)
            }
            Divider()
            SafeMarkdownText(markdown: markdown)
                .font(.body)
                .textSelection(.enabled)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(Color(uiColor: .separator))
                .frame(height: 1)
        }
    }
}

private struct BriefingRevisionHistory: View {
    let briefing: BriefingEditionData
    @Binding var isExpanded: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                let update = { isExpanded.toggle() }
                if reduceMotion {
                    update()
                } else {
                    withAnimation(.easeInOut(duration: 0.18), update)
                }
            } label: {
                HStack(spacing: 10) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Briefing history")
                            .font(.headline)
                            .foregroundStyle(StraylightTheme.ink)
                        Text(historySummary)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 0)
                    Image(systemName: "chevron.down")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(isExpanded ? 180 : 0))
                        .accessibilityHidden(true)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            .accessibilityLabel("Briefing history, \(historySummary), \(isExpanded ? "expanded" : "collapsed")")
            .accessibilityIdentifier("briefing-revision-history")

            if isExpanded {
                Divider()
                VStack(spacing: 0) {
                    ForEach(briefing.versions.reversed()) { version in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text("Version \(version.version)")
                                .font(.subheadline.weight(.semibold))
                            if version.version == briefing.currentVersion {
                                Text("CURRENT")
                                    .font(.caption2.weight(.bold))
                                    .foregroundStyle(StraylightTheme.signal)
                            }
                            Spacer(minLength: 8)
                            Text(
                                BriefingReaderDate.dateTime(
                                    version.createdAt,
                                    timeZoneIdentifier: briefing.briefing?.timezone
                                ) ?? version.createdAt
                            )
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.trailing)
                        }
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .accessibilityElement(children: .combine)
                        .accessibilityIdentifier("briefing-version-\(version.version)")

                        if version.version != briefing.versions.first?.version {
                            Divider()
                                .padding(.leading, 14)
                        }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(Color(uiColor: .separator))
                .frame(height: 1)
        }
    }

    private var historySummary: String {
        let count = briefing.versions.count
        let materialChanges = (briefing.briefing?.delta?.added.count ?? 0)
            + (briefing.briefing?.delta?.changed.count ?? 0)
            + (briefing.briefing?.delta?.removed.count ?? 0)
        let versions = "\(count) \(count == 1 ? "version" : "versions")"
        guard briefing.currentVersion > 1 else { return versions }
        let changes = "\(materialChanges) material \(materialChanges == 1 ? "change" : "changes")"
        return "\(versions) · \(changes) in the latest revision"
    }
}

private enum BriefingReaderDate {
    static func dateTime(_ value: String?, timeZoneIdentifier: String? = nil) -> String? {
        guard let value, let date = parse(value) else { return nil }
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateStyle = .medium
        formatter.timeStyle = .short
        if let timeZoneIdentifier, let timeZone = TimeZone(identifier: timeZoneIdentifier) {
            formatter.timeZone = timeZone
        }
        return formatter.string(from: date)
    }

    static func compact(_ value: String) -> String? {
        if value.count == 10 {
            let formatter = DateFormatter()
            formatter.locale = Locale(identifier: "en_US_POSIX")
            formatter.dateFormat = "yyyy-MM-dd"
            guard let date = formatter.date(from: value) else { return nil }
            formatter.locale = .current
            formatter.dateStyle = .medium
            formatter.timeStyle = .none
            return formatter.string(from: date)
        }
        return dateTime(value)
    }

    private static func parse(_ value: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: value) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: value)
    }
}
