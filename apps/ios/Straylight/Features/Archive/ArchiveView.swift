import SwiftUI

struct ArchiveView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Published editions")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(StraylightTheme.signal)
                        .textCase(.uppercase)
                    Text("Briefing archive")
                        .font(.largeTitle.bold())
                    Text("Open any date, edition, or saved revision from hosted Straylight.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                .padding(.bottom, 16)

                if let message = model.deliveryMessage {
                    Label(message, systemImage: "exclamationmark.circle")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .padding(.bottom, 12)
                }

                if model.briefingHistory.isEmpty {
                    ContentUnavailableView {
                        Label("No published briefings", systemImage: "calendar.badge.exclamationmark")
                    } description: {
                        Text("Published editions will appear here newest first.")
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 48)
                } else {
                    VStack(spacing: 0) {
                        ForEach(model.briefingHistory) { row in
                            NavigationLink {
                                ArchiveEditionView(row: row)
                            } label: {
                                ArchiveRow(row: row)
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("briefing-archive-\(row.date)-\(row.edition)")

                            if row.id != model.briefingHistory.last?.id {
                                Divider().padding(.leading, 13)
                            }
                        }
                    }
                    .background(.background, in: RoundedRectangle(cornerRadius: 8))
                    .overlay {
                        RoundedRectangle(cornerRadius: 8)
                            .stroke(StraylightTheme.line, lineWidth: 1)
                    }

                    if model.canLoadMoreBriefings {
                        Button {
                            Task { await model.loadMoreBriefings() }
                        } label: {
                            if model.isLoadingMoreBriefings {
                                ProgressView()
                                    .frame(maxWidth: .infinity, minHeight: 44)
                            } else {
                                Text("Load older briefings")
                                    .frame(maxWidth: .infinity, minHeight: 44)
                            }
                        }
                        .buttonStyle(.bordered)
                        .padding(.top, 14)
                        .disabled(model.isLoadingMoreBriefings)
                        .accessibilityIdentifier("briefing-archive-load-more")
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("briefing-archive-list")
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("Archive")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) { BrandMark() }
        }
        .refreshable {
            await model.refreshBriefingIndexAndTopics()
        }
    }
}

private struct ArchiveRow: View {
    let row: BriefingListRow

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 6) {
                Text("\(row.edition.replacingOccurrences(of: "_", with: " ").localizedCapitalized) briefing")
                    .font(.headline)
                    .foregroundStyle(StraylightTheme.ink)
                Text(row.date)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                HStack(spacing: 7) {
                    Text("\(row.itemCount) \(row.itemCount == 1 ? "item" : "items")")
                    Text("·")
                    Text("v\(row.version)")
                    if !row.sectionTitles.isEmpty {
                        Text("·")
                        Text(row.sectionTitles.prefix(2).joined(separator: ", "))
                            .lineLimit(1)
                    }
                }
                .font(.caption)
                .foregroundStyle(.tertiary)
            }
            Spacer(minLength: 8)
            Image(systemName: "chevron.right")
                .font(.caption.weight(.bold))
                .foregroundStyle(.tertiary)
                .accessibilityHidden(true)
        }
        .padding(13)
        .frame(maxWidth: .infinity, minHeight: 72, alignment: .leading)
        .contentShape(Rectangle())
    }
}

private struct ArchiveEditionView: View {
    @EnvironmentObject private var model: AppModel
    let row: BriefingListRow

    @State private var briefing: BriefingEditionData?
    @State private var selectedVersion: Int?
    @State private var errorMessage: String?
    @State private var isLoading = true

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if let briefing {
                    if briefing.versions.count > 1 {
                        Picker("Revision", selection: versionBinding) {
                            ForEach(briefing.versions.reversed()) { version in
                                Text(version.version == briefing.currentVersion
                                    ? "Version \(version.version) · Current"
                                    : "Version \(version.version)")
                                    .tag(Optional(version.version))
                            }
                        }
                        .pickerStyle(.menu)
                        .accessibilityValue("Version \(selectedVersion ?? briefing.version)")
                        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                        .padding(.horizontal, 12)
                        .background(.background, in: RoundedRectangle(cornerRadius: 8))
                        .overlay {
                            RoundedRectangle(cornerRadius: 8)
                                .stroke(StraylightTheme.line, lineWidth: 1)
                        }
                        .accessibilityIdentifier("briefing-version-selector")
                    }

                    BriefingReader(
                        briefing: briefing,
                        cachedAt: nil,
                        focusedItemID: nil
                    )
                } else if isLoading {
                    ProgressView("Loading briefing…")
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 64)
                } else {
                    ContentUnavailableView {
                        Label("Briefing unavailable", systemImage: "exclamationmark.icloud")
                    } description: {
                        Text(errorMessage ?? "This edition could not be loaded.")
                    } actions: {
                        Button("Try again") {
                            Task { await load(version: selectedVersion) }
                        }
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("\(row.edition.localizedCapitalized) · \(row.date)")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            await load(version: nil)
        }
    }

    private var versionBinding: Binding<Int?> {
        Binding(
            get: { selectedVersion ?? briefing?.version },
            set: { version in
                guard version != selectedVersion else { return }
                selectedVersion = version
                Task { await load(version: version) }
            }
        )
    }

    @MainActor
    private func load(version: Int?) async {
        isLoading = true
        errorMessage = nil
        do {
            let loaded = try await model.loadBriefing(
                date: row.date,
                edition: row.edition,
                version: version
            )
            briefing = loaded
            selectedVersion = loaded.version
        } catch {
            briefing = nil
            errorMessage = error.localizedDescription
        }
        isLoading = false
    }
}
