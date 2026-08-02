import SwiftUI

struct NewsView: View {
    @EnvironmentObject private var model: AppModel
    @State private var filter: NewsFilter = .all

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Briefing activity")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(StraylightTheme.forest)
                        .textCase(.uppercase)
                    Text("News")
                        .font(.largeTitle.bold())
                    Text("New, changed, and corrected items from the current agent briefing.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                .padding(.bottom, 16)

                Picker("News filter", selection: $filter) {
                    ForEach(NewsFilter.allCases) { value in
                        Text(value.title).tag(value)
                    }
                }
                .pickerStyle(.segmented)
                .padding(.bottom, 18)

                if visibleItems.isEmpty {
                    ContentUnavailableView {
                        Label(emptyTitle, systemImage: "newspaper")
                    } description: {
                        Text("The current structured briefing has no items in this view.")
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 48)
                } else {
                    VStack(spacing: 0) {
                        ForEach(visibleItems) { item in
                            NavigationLink {
                                NewsDetailView(news: item)
                            } label: {
                                NewsRow(
                                    news: item,
                                    isRead: model.isNewsItemRead(item.id)
                                )
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Open update")
                            .accessibilityIdentifier("news-item-\(item.id)")

                            if item.id != visibleItems.last?.id {
                                Divider()
                            }
                        }
                    }
                    .background(.background)
                    .overlay {
                        RoundedRectangle(cornerRadius: 8)
                            .stroke(StraylightTheme.line, lineWidth: 1)
                    }
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                }

                Text("News reflects published briefing activity. It does not claim APNs delivery or a durable acknowledgement receipt.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.top, 14)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("news-list")
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("News")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) { BrandMark() }
        }
    }

    private var visibleItems: [BriefingNewsItem] {
        switch filter {
        case .all:
            model.newsItems
        case .priority:
            model.newsItems.filter(\.isPriority)
        case .unread:
            model.newsItems.filter { !model.isNewsItemRead($0.id) }
        }
    }

    private var emptyTitle: String {
        switch filter {
        case .all: "No news yet"
        case .priority: "No priority updates"
        case .unread: "You’re caught up"
        }
    }
}

private enum NewsFilter: String, CaseIterable, Identifiable {
    case all
    case priority
    case unread

    var id: String { rawValue }
    var title: String { rawValue.capitalized }
}

private struct NewsRow: View {
    let news: BriefingNewsItem
    let isRead: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 7) {
                Text(news.sectionTitle.uppercased())
                    .font(.caption2.weight(.bold))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                NewsKindPill(kind: news.kind)
                Spacer(minLength: 4)
                if !isRead {
                    Circle()
                        .fill(StraylightTheme.forest)
                        .frame(width: 7, height: 7)
                        .accessibilityLabel("Unread")
                }
            }

            SafeMarkdownText(markdown: news.item.headlineMD)
                .font(.headline)
                .foregroundStyle(StraylightTheme.ink)
                .multilineTextAlignment(.leading)

            if let body = news.item.bodyMD, !body.isEmpty {
                SafeMarkdownText(markdown: body)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(3)
            }

            HStack(spacing: 5) {
                Text(DisplayDate.day(news.date))
                Text("·")
                Text(news.edition.capitalized)
                if let time = DisplayDate.time(news.deliveredAt) {
                    Text("·")
                    Text(time)
                }
            }
            .font(.caption2)
            .foregroundStyle(.tertiary)
        }
        .padding(13)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }
}

private struct NewsDetailView: View {
    @EnvironmentObject private var model: AppModel
    let news: BriefingNewsItem

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        Text(news.sectionTitle.uppercased())
                            .font(.caption.weight(.bold))
                            .foregroundStyle(.secondary)
                        NewsKindPill(kind: news.kind)
                    }
                    SafeMarkdownText(markdown: news.item.headlineMD)
                        .font(.title2.bold())
                        .foregroundStyle(StraylightTheme.ink)
                        .textSelection(.enabled)
                    Text("\(DisplayDate.day(news.date)) · \(news.edition.capitalized) · v\(news.version)")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }

                if let body = nonempty(news.item.bodyMD) {
                    SafeMarkdownText(markdown: body)
                        .font(.body)
                        .textSelection(.enabled)
                }

                if let why = nonempty(news.item.whyItMatters) {
                    DetailBlock(title: "Why it matters", text: why, tint: StraylightTheme.forest)
                }

                if let changed = nonempty(news.item.whatChanged) {
                    DetailBlock(title: "What changed", text: changed, tint: StraylightTheme.blue)
                }

                if let detail = nonempty(news.item.detailMD) {
                    DetailBlock(title: "Detail", text: detail, tint: Color.secondary)
                }

                SourcesView(item: news.item)

                Button {
                    model.markNewsItemRead(news.id)
                } label: {
                    Label(
                        model.isNewsItemRead(news.id) ? "Read" : "Mark as read",
                        systemImage: "checkmark.circle"
                    )
                    .frame(maxWidth: .infinity, minHeight: 44)
                }
                .buttonStyle(.bordered)
                .disabled(model.isNewsItemRead(news.id))
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 18)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("news-detail-\(news.id)")
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("News detail")
        .navigationBarTitleDisplayMode(.inline)
    }

    private func nonempty(_ value: String?) -> String? {
        guard let value, !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return nil
        }
        return value
    }
}

private struct DetailBlock: View {
    let title: String
    let text: String
    let tint: Color

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            Text(title.uppercased())
                .font(.caption.weight(.bold))
                .foregroundStyle(tint)
            SafeMarkdownText(markdown: text)
                .font(.body)
                .textSelection(.enabled)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(tint.opacity(0.07), in: RoundedRectangle(cornerRadius: 8))
    }
}

private struct SourcesView: View {
    let item: BriefingItem

    var body: some View {
        if !safeURLs.isEmpty || !times.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                Text("Sources")
                    .font(.headline)

                ForEach(safeURLs, id: \.absoluteString) { url in
                    Link(destination: url) {
                        Label(sourceLabel(url), systemImage: "arrow.up.right.square")
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }

                ForEach(times, id: \.0) { label, value in
                    LabeledContent(label, value: value)
                        .font(.subheadline)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.background, in: RoundedRectangle(cornerRadius: 8))
            .overlay {
                RoundedRectangle(cornerRadius: 8)
                    .stroke(StraylightTheme.line, lineWidth: 1)
            }
        }
    }

    private var safeURLs: [URL] {
        (item.story?.urls ?? []).compactMap { raw in
            guard let url = URL(string: raw), ["http", "https"].contains(url.scheme?.lowercased()) else {
                return nil
            }
            return url
        }
    }

    private var times: [(String, String)] {
        [
            ("Published", item.times?.publishedAt),
            ("Event", item.times?.eventAt),
            ("First seen", item.times?.firstSeenAt),
        ].compactMap { label, raw in
            guard let raw else { return nil }
            return (label, DisplayDate.metadata(raw))
        }
    }

    private func sourceLabel(_ url: URL) -> String {
        url.host?.replacingOccurrences(of: "www.", with: "") ?? url.absoluteString
    }
}

private struct NewsKindPill: View {
    let kind: NewsDeliveryKind

    var body: some View {
        Text(kind.label.uppercased())
            .font(.caption2.weight(.bold))
            .foregroundStyle(color)
            .padding(.horizontal, 6)
            .padding(.vertical, 3)
            .background(color.opacity(0.09), in: RoundedRectangle(cornerRadius: 5))
    }

    private var color: Color {
        switch kind {
        case .new: StraylightTheme.forest
        case .update: StraylightTheme.blue
        case .correction: StraylightTheme.red
        case .context: Color.secondary
        }
    }
}
