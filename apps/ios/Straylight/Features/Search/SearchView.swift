import SwiftUI

struct SearchView: View {
    @EnvironmentObject private var model: AppModel
    @State private var query = ""
    @State private var sort: WorkspaceSearchSort = .bestMatch

    var body: some View {
        List {
            Section {
                Text("Search returns source-backed workspace entries. Queries stay on this device only long enough to retrieve results and are never added to search history.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }

            Section {
                Picker("Sort entries", selection: $sort) {
                    ForEach(WorkspaceSearchSort.allCases) { choice in
                        Label(choice.label, systemImage: choice.symbol)
                            .tag(choice)
                    }
                }
                .pickerStyle(.menu)
                .accessibilityIdentifier("search-sort")
            }

            if let message = model.searchMessage {
                Section {
                    Label(
                        message,
                        systemImage: model.searchResults.isEmpty
                            ? "info.circle"
                            : "exclamationmark.triangle"
                    )
                    .font(.footnote)
                    .foregroundStyle(
                        model.searchResults.isEmpty ? .secondary : StraylightTheme.amber
                    )
                }
            }

            if !model.searchResults.isEmpty {
                Section("Entries") {
                    ForEach(model.searchResults) { candidate in
                        NavigationLink {
                            ContextSourceView(
                                request: WorkspaceEntryRequest(candidate: candidate)
                            )
                        } label: {
                            SearchResultRow(candidate: candidate)
                        }
                        .accessibilityIdentifier("search-result-\(candidate.id)")
                    }
                }
            } else if !model.isSearching, !query.isEmpty {
                Section {
                    BoundaryNotice(
                        symbol: "doc.text.magnifyingglass",
                        title: "No entries shown",
                        detail: "Submit the search again or broaden the wording. A partial retrieval response never proves the information is absent."
                    )
                    .listRowInsets(EdgeInsets())
                    .listRowBackground(Color.clear)
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Search")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) { BrandMark() }
            if model.isSearching {
                ToolbarItem(placement: .topBarTrailing) {
                    ProgressView()
                        .accessibilityLabel("Searching entries")
                }
            }
        }
        .searchable(
            text: $query,
            placement: .navigationBarDrawer(displayMode: .always),
            prompt: "Search durable entries"
        )
        .onSubmit(of: .search) {
            Task { await model.performSearch(query, sort: sort) }
        }
        .onChange(of: sort) { _, newSort in
            guard query.trimmingCharacters(in: .whitespacesAndNewlines).count >= 2 else {
                return
            }
            Task { await model.performSearch(query, sort: newSort) }
        }
        .onChange(of: query) { _, _ in
            model.clearSearch()
        }
        .onAppear {
            if query.isEmpty {
                model.clearSearch()
            }
        }
    }
}

private extension WorkspaceSearchSort {
    var label: String {
        switch self {
        case .bestMatch: "Best match"
        case .lastModified: "Last modified"
        case .title: "Title"
        }
    }

    var symbol: String {
        switch self {
        case .bestMatch: "sparkle.magnifyingglass"
        case .lastModified: "clock.arrow.circlepath"
        case .title: "textformat"
        }
    }
}

private struct SearchResultRow: View {
    let candidate: WorkspaceSearchCandidate

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(candidate.title)
                .font(.headline)
                .lineLimit(2)
            if let heading = candidate.heading, !heading.isEmpty {
                Text(heading)
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(StraylightTheme.signal)
                    .lineLimit(1)
            }
            Text(candidate.previewText)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .lineLimit(3)
            HStack(spacing: 6) {
                Text(candidate.path)
                    .lineLimit(1)
                if let version = candidate.version {
                    Text("v\(version)")
                }
                if let updatedAt = candidate.updatedAt {
                    Text("Modified \(DisplayDate.metadata(updatedAt))")
                        .lineLimit(1)
                }
            }
            .font(.caption2)
            .foregroundStyle(.tertiary)
        }
        .padding(.vertical, 4)
    }
}

struct ContextSourceView: View {
    let request: WorkspaceEntryRequest
    @EnvironmentObject private var model: AppModel
    @State private var item: WorkspaceReadItem?
    @State private var errorMessage: String?
    @State private var usesMarkdownFormatting = true
    @State private var linkedRequest: WorkspaceEntryRequest?
    @State private var presentsLinkedEntry = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Eyebrow(text: item?.version == nil ? "Exact entry" : "Pinned entry")
                Text(item?.title ?? request.title)
                    .font(.title.bold())
                    .fixedSize(horizontal: false, vertical: true)

                if let path = item?.path ?? request.pathCandidates.first {
                    Text(path)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }

                Divider()

                if let item, let text = item.text {
                    HStack(spacing: 8) {
                        if let version = item.version {
                            StatusPill(text: "Pinned v\(version)", color: StraylightTheme.signal)
                        }
                        if item.truncated == true {
                            StatusPill(text: "Bounded read", color: StraylightTheme.amber)
                        }
                    }

                    if let updatedAt = item.updatedAt {
                        Text("Last modified \(DisplayDate.metadata(updatedAt))")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }

                    HStack(spacing: 12) {
                        Label(
                            usesMarkdownFormatting ? "Formatted Markdown" : "Raw Markdown",
                            systemImage: usesMarkdownFormatting
                                ? "text.document.fill"
                                : "chevron.left.forwardslash.chevron.right"
                        )
                        .font(.subheadline.weight(.medium))
                        Spacer(minLength: 8)
                        Toggle("Markdown formatting", isOn: $usesMarkdownFormatting)
                            .labelsHidden()
                            .accessibilityIdentifier("entry-markdown-toggle")
                    }
                    .padding(12)
                    .background(.background, in: RoundedRectangle(cornerRadius: 8))
                    .overlay {
                        RoundedRectangle(cornerRadius: 8)
                            .stroke(StraylightTheme.line, lineWidth: 1)
                    }

                    if usesMarkdownFormatting {
                        EntryMarkdownText(
                            markdown: text,
                            onEntryLink: { link in
                                linkedRequest = WorkspaceEntryRequest(
                                    link: link,
                                    sourcePath: item.path
                                )
                                presentsLinkedEntry = true
                            }
                        )
                        .accessibilityIdentifier("entry-formatted-content")
                    } else {
                        Text(text)
                            .font(.system(.body, design: .monospaced))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .accessibilityIdentifier("entry-raw-content")
                    }
                } else if let errorMessage {
                    BoundaryNotice(
                        symbol: "exclamationmark.icloud",
                        title: "Entry unavailable",
                        detail: errorMessage
                    )
                } else {
                    ProgressView("Reading exact entry…")
                        .frame(maxWidth: .infinity, minHeight: 160)
                }
            }
            .padding(16)
            .frame(maxWidth: 720)
            .frame(maxWidth: .infinity)
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("Entry")
        .navigationBarTitleDisplayMode(.inline)
        .navigationDestination(isPresented: $presentsLinkedEntry) {
            if let linkedRequest {
                ContextSourceView(request: linkedRequest)
            }
        }
        .task(id: request) {
            item = nil
            errorMessage = nil
            do {
                item = try await model.read(request)
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }
}

private struct EntryMarkdownText: View {
    let markdown: String
    let onEntryLink: (WorkspaceEntryLink) -> Void

    var body: some View {
        Text(SafeMarkdown.entryAttributedString(markdown))
            .tint(StraylightTheme.signal)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .environment(\.openURL, OpenURLAction { url in
                if let link = EntryNavigationURL.link(from: url) {
                    onEntryLink(link)
                    return .handled
                }
                let scheme = url.scheme?.lowercased()
                return scheme == "https" || scheme == "http" ? .systemAction : .discarded
            })
    }
}
