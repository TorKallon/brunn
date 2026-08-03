import SwiftUI

struct SearchView: View {
    @EnvironmentObject private var model: AppModel
    @State private var query = ""

    var body: some View {
        List {
            Section {
                Text("Search returns source-backed workspace matches. The MVP does not add a generated-answer layer or persist your query history.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }

            if let message = model.searchMessage {
                Section {
                    Label(message, systemImage: model.searchResults.isEmpty ? "info.circle" : "exclamationmark.triangle")
                        .font(.footnote)
                        .foregroundStyle(model.searchResults.isEmpty ? .secondary : StraylightTheme.amber)
                }
            }

            if !model.searchResults.isEmpty {
                Section("Sources") {
                    ForEach(model.searchResults) { candidate in
                        NavigationLink {
                            ContextSourceView(candidate: candidate)
                        } label: {
                            SearchResultRow(candidate: candidate)
                        }
                    }
                }
            } else if !model.isSearching, !query.isEmpty {
                Section {
                    BoundaryNotice(
                        symbol: "doc.text.magnifyingglass",
                        title: "No sources shown",
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
                ToolbarItem(placement: .topBarTrailing) { ProgressView() }
            }
        }
        .searchable(text: $query, prompt: "Search durable context")
        .onSubmit(of: .search) {
            Task { await model.performSearch(query) }
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
            }
            .font(.caption2)
            .foregroundStyle(.tertiary)
        }
        .padding(.vertical, 4)
    }
}

private struct ContextSourceView: View {
    let candidate: WorkspaceSearchCandidate
    @EnvironmentObject private var model: AppModel
    @State private var item: WorkspaceReadItem?
    @State private var errorMessage: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Eyebrow(text: "Exact source")
                Text(candidate.title)
                    .font(.title.bold())
                Text(candidate.path)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)

                Divider()

                if let item, let text = item.text {
                    HStack(spacing: 8) {
                        StatusPill(text: "Raw Markdown", color: StraylightTheme.pulse)
                        if let version = item.version {
                            StatusPill(text: "Pinned v\(version)", color: StraylightTheme.signal)
                        }
                    }
                    Text(text)
                        .font(.system(.body, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if item.truncated == true {
                        StatusPill(text: "Bounded read", color: StraylightTheme.amber)
                    }
                } else if let errorMessage {
                    BoundaryNotice(
                        symbol: "exclamationmark.icloud",
                        title: "Source unavailable",
                        detail: errorMessage
                    )
                } else {
                    ProgressView("Reading exact source…")
                        .frame(maxWidth: .infinity, minHeight: 160)
                }
            }
            .padding(16)
            .frame(maxWidth: 720)
            .frame(maxWidth: .infinity)
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("Source")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            do {
                item = try await model.read(candidate)
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }
}
