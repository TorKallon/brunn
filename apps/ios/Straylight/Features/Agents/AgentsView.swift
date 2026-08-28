import SwiftUI

typealias MessagingConversationCreator = @MainActor (
    _ participants: [String],
    _ subject: String?
) async throws -> String

@MainActor
struct AgentsView: View {
    @ObservedObject private var controller: MessagingController

    private let canWriteMessages: Bool
    private let focusedConversationID: String?
    private let focusedSequence: Int64?
    private let createConversationOverride: MessagingConversationCreator?

    @State private var navigationConversationID: String?
    @State private var navigationFocusedConversationID: String?
    @State private var navigationFocusedSequence: Int64?
    @State private var presentsConversation = false
    @State private var presentsPicker = false
    @State private var handledFocusKey: String?

    init(
        controller: MessagingController,
        canWriteMessages: Bool,
        focusedConversationID: String? = nil,
        focusedSequence: Int64? = nil,
        onCreateConversation: MessagingConversationCreator? = nil
    ) {
        _controller = ObservedObject(wrappedValue: controller)
        self.canWriteMessages = canWriteMessages
        self.focusedConversationID = focusedConversationID
        self.focusedSequence = focusedSequence
        createConversationOverride = onCreateConversation
    }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                header

                if controller.lastTransportError != nil {
                    Label(
                        "Offline · showing saved conversations",
                        systemImage: "icloud.slash"
                    )
                    .font(.footnote)
                    .foregroundStyle(StraylightTheme.amber)
                    .accessibilityIdentifier("messaging-offline-notice")
                }

                conversationSection
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("messaging-conversation-list")

                agentSection
            }
            .frame(maxWidth: 720)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14)
            .padding(.top, 16)
            .padding(.bottom, 36)
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("Agents")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) { BrandMark() }
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    presentsPicker = true
                } label: {
                    Label("New conversation", systemImage: "square.and.pencil")
                }
                .disabled(!canStartConversation)
                .accessibilityIdentifier("messaging-new-conversation")
            }
        }
        .refreshable {
            _ = try? await controller.refreshInbox()
        }
        .task {
            _ = try? await controller.refreshInbox()
            openFocusedConversationIfAvailable()
        }
        .onChange(of: focusedConversationID) { _, _ in
            openFocusedConversationIfAvailable()
        }
        .onChange(of: focusedSequence) { _, _ in
            openFocusedConversationIfAvailable()
        }
        .onChange(of: controller.conversations.count) { _, _ in
            openFocusedConversationIfAvailable()
        }
        .onChange(of: presentsConversation) { _, isPresented in
            if !isPresented {
                navigationConversationID = nil
                navigationFocusedConversationID = nil
                navigationFocusedSequence = nil
            }
        }
        .navigationDestination(isPresented: $presentsConversation) {
            if let conversationID = navigationConversationID {
                ConversationView(
                    controller: controller,
                    conversationID: conversationID,
                    canWriteMessages: canWriteMessages,
                    focusedConversationID: navigationFocusedConversationID,
                    focusedSequence: navigationFocusedSequence
                )
            }
        }
        .sheet(isPresented: $presentsPicker) {
            NavigationStack {
                AgentPickerView(
                    agents: availableAgents,
                    onCreate: createConversation,
                    onCreated: { conversationID in
                        presentsPicker = false
                        openConversation(conversationID)
                    }
                )
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("messaging-agents-root")
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow(text: "Durable mailbox")
            Text("Agent conversations")
                .font(.largeTitle.bold())
                .foregroundStyle(StraylightTheme.ink)
            Text("Messages stay queued until each participant is ready to read them.")
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    @ViewBuilder
    private var conversationSection: some View {
        let summaries = logicalConversations
        VStack(alignment: .leading, spacing: 10) {
            sectionHeader(
                title: "Conversations",
                value: summaries.count.formatted()
            )

            if summaries.isEmpty {
                ContentUnavailableView {
                    Label("No conversations yet", systemImage: "bubble.left.and.bubble.right")
                } description: {
                    Text(canWriteMessages
                        ? "Start a direct or group conversation with a registered agent."
                        : "Conversations will appear here after an agent sends a message.")
                }
                .frame(maxWidth: .infinity, minHeight: 180)
                .background(.background, in: RoundedRectangle(cornerRadius: 8))
                .overlay {
                    RoundedRectangle(cornerRadius: 8)
                        .stroke(StraylightTheme.line, lineWidth: 1)
                }
            } else {
                VStack(spacing: 0) {
                    ForEach(summaries, id: \.root.conversationID) { summary in
                        Button {
                            openConversation(summary.root.conversationID)
                        } label: {
                            MessagingConversationRow(
                                summary: summary,
                                agentsByID: agentsByID,
                                ownerAgentID: ownerAgentID
                            )
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier(
                            "messaging-conversation-\(summary.root.conversationID)"
                        )

                        if summary.root.conversationID
                            != summaries.last?.root.conversationID
                        {
                            Divider().padding(.leading, 58)
                        }
                    }
                }
                .background(.background, in: RoundedRectangle(cornerRadius: 8))
                .overlay {
                    RoundedRectangle(cornerRadius: 8)
                        .stroke(StraylightTheme.line, lineWidth: 1)
                }
                .clipShape(RoundedRectangle(cornerRadius: 8))
            }
        }
    }

    @ViewBuilder
    private var agentSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            sectionHeader(title: "Principals", value: availableAgents.count.formatted())

            if availableAgents.isEmpty {
                Text("No active messaging principals are registered.")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 64, alignment: .leading)
                    .padding(.horizontal, 14)
                    .background(.background, in: RoundedRectangle(cornerRadius: 8))
            } else {
                VStack(spacing: 0) {
                    ForEach(availableAgents, id: \.agentID) { agent in
                        MessagingAgentPresenceRow(agent: agent)
                            .padding(.horizontal, 14)
                            .padding(.vertical, 11)
                            .accessibilityIdentifier("messaging-agent-\(agent.agentID)")
                        if agent.agentID != availableAgents.last?.agentID {
                            Divider().padding(.leading, 58)
                        }
                    }
                }
                .background(.background, in: RoundedRectangle(cornerRadius: 8))
                .overlay {
                    RoundedRectangle(cornerRadius: 8)
                        .stroke(StraylightTheme.line, lineWidth: 1)
                }
                .clipShape(RoundedRectangle(cornerRadius: 8))
            }
        }
    }

    private func sectionHeader(title: String, value: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title)
                .font(.headline)
                .foregroundStyle(StraylightTheme.ink)
            Spacer()
            Text(value)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
        }
    }

    private var availableAgents: [MessagingAgentRecord] {
        controller.agents.filter { !$0.archived }
    }

    private var agentsByID: [String: MessagingAgentRecord] {
        Dictionary(uniqueKeysWithValues: controller.agents.map { ($0.agentID, $0) })
    }

    private var ownerAgentID: String? {
        let owners = availableAgents.filter { $0.principalKind == "owner" }
        return owners.count == 1 ? owners[0].agentID : nil
    }

    private var canStartConversation: Bool {
        canWriteMessages && ownerAgentID != nil
            && availableAgents.contains { $0.principalKind != "owner" }
    }

    private var logicalConversations: [LogicalConversationSummary] {
        let conversationsByID = Dictionary(
            uniqueKeysWithValues: controller.conversations.map {
                ($0.conversationID, $0)
            }
        )
        var seenRoots = Set<String>()
        var summaries: [LogicalConversationSummary] = []

        for conversation in controller.conversations {
            let rootID = (try? controller.logicalRootConversationID(
                for: conversation.conversationID
            )) ?? conversation.conversationID
            guard seenRoots.insert(rootID).inserted else { continue }

            let memberIDs = (try? controller.conversationChain(containing: rootID))
                ?? [rootID]
            let members = memberIDs.compactMap { conversationsByID[$0] }
            guard let root = conversationsByID[rootID] ?? members.first else {
                continue
            }
            summaries.append(LogicalConversationSummary(
                root: root,
                members: members.isEmpty ? [root] : members,
                messages: (try? controller.messages(conversationID: rootID)) ?? []
            ))
        }

        return summaries.sorted { left, right in
            if left.needsHuman != right.needsHuman {
                return left.needsHuman && !right.needsHuman
            }
            if left.lastMessageAt != right.lastMessageAt {
                return (left.lastMessageAt ?? "") > (right.lastMessageAt ?? "")
            }
            return left.root.conversationID < right.root.conversationID
        }
    }

    private func openConversation(
        _ conversationID: String,
        focusedConversationID: String? = nil,
        focusedSequence: Int64? = nil
    ) {
        navigationConversationID = conversationID
        navigationFocusedConversationID = focusedConversationID
        navigationFocusedSequence = focusedSequence
        presentsConversation = true
    }

    private func openFocusedConversationIfAvailable() {
        guard let focusedConversationID else { return }
        let focusKey = "\(focusedConversationID)#\(focusedSequence ?? 0)"
        guard handledFocusKey != focusKey,
              controller.conversations.contains(where: {
                  $0.conversationID == focusedConversationID
              })
        else { return }
        let rootID = (try? controller.logicalRootConversationID(
            for: focusedConversationID
        )) ?? focusedConversationID
        handledFocusKey = focusKey
        openConversation(
            rootID,
            focusedConversationID: focusedConversationID,
            focusedSequence: focusedSequence
        )
    }

    private func createConversation(
        participants: [String],
        subject: String?
    ) async throws -> String {
        if let createConversationOverride {
            return try await createConversationOverride(participants, subject)
        }
        return try await controller.createConversation(
            participants: participants,
            subject: subject
        ).conversationID
    }
}

private struct LogicalConversationSummary {
    let root: MessagingConversationRecord
    let members: [MessagingConversationRecord]
    let messages: [MessagingMessageRecord]

    var active: MessagingConversationRecord { members.last ?? root }
    var needsHuman: Bool { members.contains(where: \.needsHuman) }
    var unreadCount: Int64 { members.reduce(0) { $0 + max($1.unreadCount, 0) } }
    var queuedCount: Int {
        messages.count { $0.deliveryState == .queued }
    }
    var lastMessage: MessagingMessageRecord? { messages.last }
    var lastMessageAt: String? {
        members.compactMap(\.lastMessageAt).max() ?? lastMessage?.createdAt
    }
}

@MainActor
private struct MessagingConversationRow: View {
    let summary: LogicalConversationSummary
    let agentsByID: [String: MessagingAgentRecord]
    let ownerAgentID: String?

    private var conversation: MessagingConversationRecord { summary.root }

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            ZStack(alignment: .bottomTrailing) {
                Image(systemName: conversation.conversationKind == "group"
                    ? "person.2.fill"
                    : "bubble.left.fill")
                    .font(.headline)
                    .foregroundStyle(StraylightTheme.signal)
                    .frame(width: 40, height: 40)
                    .background(
                        StraylightTheme.signal.opacity(0.10),
                        in: RoundedRectangle(cornerRadius: 8)
                    )

                if hasOnlineParticipant {
                    Circle()
                        .fill(StraylightTheme.success)
                        .frame(width: 11, height: 11)
                        .overlay { Circle().stroke(.background, lineWidth: 2) }
                        .accessibilityHidden(true)
                }
            }

            VStack(alignment: .leading, spacing: 5) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(title)
                        .font(.headline)
                        .foregroundStyle(StraylightTheme.ink)
                        .lineLimit(2)
                    Spacer(minLength: 6)
                    if let lastMessageAt = summary.lastMessageAt {
                        Text(DisplayDate.metadata(lastMessageAt))
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                }

                Text(lastMessageSnippet)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)

                HStack(spacing: 7) {
                    Label(
                        presenceLabel,
                        systemImage: hasOnlineParticipant ? "circle.fill" : "clock"
                    )
                    .foregroundStyle(hasOnlineParticipant
                        ? StraylightTheme.success
                        : Color.secondary)

                    if summary.needsHuman {
                        Label("Needs you", systemImage: "person.crop.circle.badge.exclamationmark")
                            .foregroundStyle(StraylightTheme.amber)
                    } else if summary.active.status != "open" {
                        Label(statusLabel, systemImage: statusSymbol)
                            .foregroundStyle(StraylightTheme.amber)
                    }

                    if summary.unreadCount > 0 {
                        Text("\(summary.unreadCount.formatted()) unread")
                            .foregroundStyle(StraylightTheme.signal)
                            .padding(.horizontal, 7)
                            .padding(.vertical, 2)
                            .background(
                                StraylightTheme.signal.opacity(0.12),
                                in: Capsule()
                            )
                    }

                    if summary.queuedCount > 0 {
                        Text("\(summary.queuedCount.formatted()) queued")
                            .foregroundStyle(StraylightTheme.amber)
                            .padding(.horizontal, 7)
                            .padding(.vertical, 2)
                            .background(
                                StraylightTheme.amber.opacity(0.12),
                                in: Capsule()
                            )
                    }
                }
                .font(.caption.weight(.semibold))
            }

            Image(systemName: "chevron.right")
                .font(.caption.weight(.bold))
                .foregroundStyle(.tertiary)
                .padding(.top, 7)
                .accessibilityHidden(true)
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 72, alignment: .leading)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityLabel(accessibilityLabel)
    }

    private var participantIDs: [String] {
        conversation.participants
            .map(\.agentID)
            .filter { $0 != ownerAgentID }
    }

    private var title: String {
        if let subject = conversation.subject?.trimmingCharacters(in: .whitespacesAndNewlines),
           !subject.isEmpty
        {
            return subject
        }
        let names = participantIDs.map { agentsByID[$0]?.displayName ?? $0 }
        return names.isEmpty ? "Conversation" : names.joined(separator: ", ")
    }

    private var participantSummary: String {
        let names = participantIDs.map { agentsByID[$0]?.displayName ?? $0 }
        if names.isEmpty { return "Owner conversation" }
        return names.joined(separator: " · ")
    }

    private var lastMessageSnippet: String {
        guard let message = summary.lastMessage else { return participantSummary }
        let body = message.bodyMarkdown
            .split(whereSeparator: { $0.isWhitespace })
            .joined(separator: " ")
        if !body.isEmpty { return body }
        return message.kind == "system" ? "System update" : participantSummary
    }

    private var hasOnlineParticipant: Bool {
        participantIDs.contains { agentsByID[$0]?.online == true }
    }

    private var presenceLabel: String {
        if hasOnlineParticipant { return "Online" }
        let mostRecent = participantIDs
            .compactMap { agentsByID[$0]?.lastSeenAt }
            .max()
        if let mostRecent { return "Last seen \(DisplayDate.metadata(mostRecent))" }
        return "Offline"
    }

    private var statusLabel: String {
        switch summary.active.status {
        case "paused_for_human": "Paused"
        case "closed": "Closed"
        default: summary.active.status.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }

    private var statusSymbol: String {
        summary.active.status == "closed" ? "lock" : "pause.circle"
    }

    private var accessibilityLabel: String {
        var parts = [title, presenceLabel]
        parts.append(lastMessageSnippet)
        if summary.unreadCount > 0 {
            parts.append("\(summary.unreadCount) unread")
        }
        if summary.queuedCount > 0 {
            parts.append("\(summary.queuedCount) queued")
        }
        if summary.needsHuman { parts.append("Needs you") }
        if summary.active.status != "open" { parts.append(statusLabel) }
        return parts.joined(separator: ", ")
    }
}

@MainActor
struct MessagingAgentPresenceRow: View {
    let agent: MessagingAgentRecord

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            Image(systemName: agent.principalKind == "owner" ? "person.fill" : "sparkles")
                .font(.headline)
                .foregroundStyle(agent.online ? StraylightTheme.success : StraylightTheme.signal)
                .frame(width: 34, height: 34)
                .background(
                    (agent.online ? StraylightTheme.success : StraylightTheme.signal).opacity(0.10),
                    in: RoundedRectangle(cornerRadius: 8)
                )

            VStack(alignment: .leading, spacing: 3) {
                Text(agent.displayName)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(StraylightTheme.ink)
                Text(agent.agentID)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer(minLength: 8)

            Label(statusText, systemImage: agent.online ? "circle.fill" : "clock")
                .font(.caption.weight(.semibold))
                .foregroundStyle(agent.online ? StraylightTheme.success : Color.secondary)
                .multilineTextAlignment(.trailing)
        }
        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(agent.displayName), \(statusText)")
    }

    private var statusText: String {
        if agent.online { return "Online" }
        if let lastSeenAt = agent.lastSeenAt {
            return "Last seen \(DisplayDate.metadata(lastSeenAt))"
        }
        return "Offline"
    }
}
