import SwiftUI

struct TopicsView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        List {
            if let snapshot = model.topicsSnapshot {
                Section {
                    ForEach(snapshot.topics) { topic in
                        NavigationLink {
                            TopicDetailView(topic: topic)
                        } label: {
                            VStack(alignment: .leading, spacing: 6) {
                                HStack(alignment: .firstTextBaseline) {
                                    Text(topic.name)
                                        .font(.headline)
                                    Spacer(minLength: 8)
                                    Text(modeLabel(topic.mode))
                                        .font(.caption.weight(.semibold))
                                        .foregroundStyle(modeColor(topic.mode))
                                }
                                Text(topic.body.isEmpty ? "No agent instructions." : topic.body)
                                    .font(.subheadline)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(2)
                                Text(topic.editions.map(\.localizedCapitalized).joined(separator: " · "))
                                    .font(.caption)
                                    .foregroundStyle(.tertiary)
                            }
                            .padding(.vertical, 4)
                        }
                    }
                } header: {
                    Text("Tracked topics")
                } footer: {
                    Text("Topics are canonical Straylight records used by briefing agents. This read-only mobile MVP never creates a second preference store.")
                }

                if !snapshot.pendingRequests.isEmpty {
                    Section("Pending deep-dives") {
                        ForEach(snapshot.pendingRequests) { request in
                            VStack(alignment: .leading, spacing: 5) {
                                Text(request.topic?.localizedCapitalized ?? "Briefing request")
                                    .font(.headline)
                                if let note = request.note, !note.isEmpty {
                                    Text(note)
                                        .font(.subheadline)
                                }
                                HStack(spacing: 6) {
                                    if let date = request.date { Text(date) }
                                    if let itemID = request.itemID { Text("· \(itemID)") }
                                }
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            }
                            .padding(.vertical, 4)
                        }
                        if snapshot.pendingRequestsTruncated == true {
                            Label("Additional pending requests exist in Straylight.", systemImage: "ellipsis.circle")
                                .font(.footnote)
                        }
                    }
                }
            } else {
                ContentUnavailableView {
                    Label("Topics unavailable", systemImage: "scope")
                } description: {
                    Text("Pull to retry the current topic snapshot.")
                }
            }
        }
        .navigationTitle("Tracked topics")
        .refreshable {
            await model.refreshBriefingIndexAndTopics()
        }
    }

    private func modeLabel(_ mode: String) -> String {
        switch mode {
        case "every_briefing": "Every briefing"
        case "on_material_delta": "Material changes"
        case "scheduled": "Scheduled"
        case "paused": "Paused"
        case "muted": "Muted"
        default: mode.replacingOccurrences(of: "_", with: " ").localizedCapitalized
        }
    }

    private func modeColor(_ mode: String) -> Color {
        switch mode {
        case "paused", "muted": .secondary
        case "scheduled": StraylightTheme.blue
        default: StraylightTheme.forest
        }
    }
}

private struct TopicDetailView: View {
    let topic: BriefingTopic

    var body: some View {
        List {
            Section {
                LabeledContent("Mode", value: topic.mode.replacingOccurrences(of: "_", with: " ").localizedCapitalized)
                LabeledContent("Editions", value: topic.editions.map(\.localizedCapitalized).joined(separator: ", "))
                LabeledContent("Freshness", value: "\(topic.freshnessHours) hours")
                LabeledContent("Order", value: String(topic.sectionOrder))
                if let schedule = topic.schedule, !schedule.isEmpty {
                    LabeledContent("Schedule", value: schedule)
                }
                LabeledContent("Suppress unchanged", value: topic.suppressUnchanged ? "Yes" : "No")
            } header: {
                Text("Delivery")
            }

            if !topic.entities.isEmpty || !topic.symbols.isEmpty {
                Section("Tracking") {
                    if !topic.entities.isEmpty {
                        LabeledContent("Entities", value: topic.entities.joined(separator: ", "))
                    }
                    if !topic.symbols.isEmpty {
                        LabeledContent("Symbols", value: topic.symbols.joined(separator: ", "))
                    }
                }
            }

            Section("Agent instructions") {
                Text(topic.body.isEmpty ? "No instructions." : topic.body)
                    .textSelection(.enabled)
                if topic.truncated == true {
                    Label("Instructions were truncated by the snapshot limit.", systemImage: "ellipsis.circle")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                if let parseError = topic.parseError {
                    Label(parseError, systemImage: "exclamationmark.triangle")
                        .font(.footnote)
                        .foregroundStyle(StraylightTheme.red)
                }
            }
        }
        .navigationTitle(topic.name)
        .navigationBarTitleDisplayMode(.inline)
    }
}
