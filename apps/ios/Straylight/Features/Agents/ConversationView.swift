import SwiftUI

@MainActor
struct ConversationView: View {
    @ObservedObject private var controller: MessagingController
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.scenePhase) private var scenePhase

    let conversationID: String
    let canWriteMessages: Bool
    let focusedConversationID: String?
    let focusedSequence: Int64?

    @State private var draft = ""
    @State private var composesQuestion = false
    @State private var sendMessage: String?
    @FocusState private var composerFocused: Bool

    init(
        controller: MessagingController,
        conversationID: String,
        canWriteMessages: Bool,
        focusedConversationID: String? = nil,
        focusedSequence: Int64? = nil
    ) {
        _controller = ObservedObject(wrappedValue: controller)
        self.conversationID = conversationID
        self.canWriteMessages = canWriteMessages
        self.focusedConversationID = focusedConversationID
        self.focusedSequence = focusedSequence
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    threadState

                    Text("\(messages.count.formatted()) messages")
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                        .accessibilityLabel("\(messages.count) loaded messages")
                        .accessibilityIdentifier("messaging-thread-message-count")

                    if messages.isEmpty {
                        ContentUnavailableView {
                            Label("No messages yet", systemImage: "bubble.left")
                        } description: {
                            Text(canCompose
                                ? "Send a short message to begin this durable thread."
                                : "Messages will appear here when a participant writes.")
                        }
                        .frame(maxWidth: .infinity, minHeight: 220)
                    } else {
                        ForEach(messages, id: \.storageKey) { message in
                            MessagingBubble(
                                message: message,
                                isOwner: message.fromAgentID == ownerAgentID,
                                senderName: senderName(for: message.fromAgentID)
                            )
                            .id(scrollID(for: message))
                        }
                    }

                    Color.clear
                        .frame(height: 1)
                        .id(bottomID)
                        .accessibilityHidden(true)
                }
                .frame(maxWidth: 720)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 12)
                .padding(.top, 12)
                .padding(.bottom, 10)
            }
            .scrollDismissesKeyboard(.interactively)
            .background(StraylightTheme.canvas)
            .accessibilityIdentifier("messaging-thread-scroll")
            .onAppear {
                focusThread(using: proxy)
            }
            .onChange(of: focusedSequence) { _, _ in
                focusThread(using: proxy)
            }
            .onChange(of: focusedConversationID) { _, _ in
                focusThread(using: proxy)
            }
            .onChange(of: messages.count) { oldCount, newCount in
                if focusedSequence == nil, newCount > oldCount {
                    scroll(using: proxy, to: bottomID, anchor: .bottom)
                } else {
                    focusThread(using: proxy)
                }
            }
            .safeAreaInset(edge: .bottom, spacing: 0) {
                composer
            }
        }
        .navigationTitle(conversationTitle)
        .navigationBarTitleDisplayMode(.inline)
        .task(id: "\(logicalConversationID):\(canWriteMessages)") {
            do {
                try controller.selectConversation(logicalConversationID)
            } catch {
                sendMessage = "The saved thread could not be opened."
                return
            }
            await pollVisibleConversation()
        }
        .onChange(of: scenePhase) { _, phase in
            guard phase == .active else { return }
            Task {
                _ = try? await controller.refreshThread(
                    conversationID: logicalConversationID
                )
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityValue("\(messages.count) loaded messages")
        .accessibilityIdentifier("messaging-thread-\(logicalConversationID)")
    }

    @ViewBuilder
    private var threadState: some View {
        if ownerAgentID == nil {
            Label(
                "The owner messaging principal is unavailable.",
                systemImage: "person.crop.circle.badge.exclamationmark"
            )
            .font(.footnote)
            .foregroundStyle(StraylightTheme.amber)
        } else if activeConversation?.status == "closed" {
            Label("This conversation is closed.", systemImage: "lock")
                .font(.footnote)
                .foregroundStyle(.secondary)
        } else if logicalConversations.contains(where: \.needsHuman)
            || activeConversation?.status == "paused_for_human"
        {
            Label(
                "Agents paused this thread for your attention. Your reply resumes it.",
                systemImage: "person.crop.circle.badge.exclamationmark"
            )
            .font(.footnote)
            .foregroundStyle(StraylightTheme.amber)
        }

        if controller.lastTransportError != nil {
            Label(
                "Offline · unsent messages stay queued",
                systemImage: "clock.arrow.circlepath"
            )
            .font(.footnote)
            .foregroundStyle(StraylightTheme.amber)
        }

        if let sendMessage {
            Label(sendMessage, systemImage: "exclamationmark.triangle")
                .font(.footnote)
                .foregroundStyle(StraylightTheme.red)
                .accessibilityIdentifier("messaging-compose-message")
        }
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: 8) {
            if !canWriteMessages {
                Label(
                    "View only · use More → Settings to add secure message access",
                    systemImage: "lock"
                )
                .font(.footnote)
                .foregroundStyle(StraylightTheme.amber)
                .accessibilityLabel(
                    "View only. Open More, then Settings, to add secure message access."
                )
                .accessibilityIdentifier("messaging-view-only")
            }

            if isClosed {
                Label(
                    "Closed · this conversation is read only",
                    systemImage: "lock"
                )
                .font(.footnote)
                .foregroundStyle(StraylightTheme.amber)
                .accessibilityIdentifier("messaging-closed")
            }

            if draft.utf8.count > 16_384 {
                Label("Message is over the 16 KiB limit", systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(StraylightTheme.red)
            }

            HStack(alignment: .bottom, spacing: 8) {
                TextField("Message agents", text: $draft, axis: .vertical)
                    .lineLimit(1 ... 6)
                    .focused($composerFocused)
                    .textInputAutocapitalization(.sentences)
                    .autocorrectionDisabled(false)
                    .padding(.horizontal, 11)
                    .padding(.vertical, 10)
                    .background(.background, in: RoundedRectangle(cornerRadius: 8))
                    .overlay {
                        RoundedRectangle(cornerRadius: 8)
                            .stroke(
                                composerFocused ? StraylightTheme.signal : StraylightTheme.line,
                                lineWidth: composerFocused ? 2 : 1
                            )
                    }
                    .disabled(!canCompose)
                    .accessibilityIdentifier("messaging-composer")

                Button {
                    composesQuestion.toggle()
                } label: {
                    Image(systemName: composesQuestion
                        ? "questionmark.bubble.fill"
                        : "questionmark.bubble")
                        .frame(width: 36, height: 36)
                }
                .buttonStyle(.bordered)
                .tint(composesQuestion ? StraylightTheme.signal : .secondary)
                .disabled(!canCompose)
                .accessibilityLabel(composesQuestion ? "Question selected" : "Send as question")
                .accessibilityAddTraits(composesQuestion ? .isSelected : [])
                .accessibilityIdentifier("messaging-compose-question")

                Button(action: sendDraft) {
                    Image(systemName: "arrow.up")
                        .font(.body.weight(.bold))
                        .frame(width: 36, height: 36)
                }
                .buttonStyle(.borderedProminent)
                .tint(StraylightTheme.signal)
                .disabled(!canSend)
                .accessibilityLabel("Send message")
                .accessibilityIdentifier("messaging-send")
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 10)
        .padding(.bottom, 8)
        .background(.bar)
    }

    private var conversation: MessagingConversationRecord? {
        controller.conversations.first { $0.conversationID == logicalConversationID }
    }

    private var logicalConversationID: String {
        (try? controller.logicalRootConversationID(for: conversationID)) ?? conversationID
    }

    private var logicalConversationIDs: [String] {
        (try? controller.conversationChain(containing: logicalConversationID))
            ?? [logicalConversationID]
    }

    private var logicalConversations: [MessagingConversationRecord] {
        let ids = Set(logicalConversationIDs)
        return controller.conversations.filter { ids.contains($0.conversationID) }
    }

    private var activeConversation: MessagingConversationRecord? {
        let activeID = (try? controller.activeConversationID(
            containing: logicalConversationID
        )) ?? logicalConversationID
        return controller.conversations.first { $0.conversationID == activeID }
    }

    private var messages: [MessagingMessageRecord] {
        guard controller.selectedConversationID == logicalConversationID else {
            return (try? controller.messages(conversationID: logicalConversationID)) ?? []
        }
        return controller.selectedMessages
    }

    private var ownerAgentID: String? {
        let owners = controller.agents.filter {
            !$0.archived && $0.principalKind == "owner"
        }
        return owners.count == 1 ? owners[0].agentID : nil
    }

    private var canCompose: Bool {
        canWriteMessages && ownerAgentID != nil && !isClosed
    }

    private var isClosed: Bool {
        activeConversation?.status == "closed"
    }

    private var canSend: Bool {
        canCompose
            && !controller.isMutating
            && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && draft.utf8.count <= 16_384
    }

    private var conversationTitle: String {
        if let subject = conversation?.subject?.trimmingCharacters(in: .whitespacesAndNewlines),
           !subject.isEmpty
        {
            return subject
        }
        let participants = conversation?.participants
            .map(\.agentID)
            .filter { $0 != ownerAgentID }
            .map { id in controller.agents.first { $0.agentID == id }?.displayName ?? id }
            ?? []
        return participants.isEmpty ? "Conversation" : participants.joined(separator: ", ")
    }

    private func senderName(for agentID: String?) -> String {
        guard let agentID else { return "System" }
        return controller.agents.first { $0.agentID == agentID }?.displayName ?? agentID
    }

    private func sendDraft() {
        guard canSend, let ownerAgentID else { return }
        let body = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        let kind: MessagingWritableMessageKind = composesQuestion ? .question : .text
        draft = ""
        composesQuestion = false
        sendMessage = nil
        composerFocused = false

        Task {
            do {
                let activeID = (try? controller.activeConversationID(
                    containing: logicalConversationID
                )) ?? logicalConversationID
                _ = try await controller.send(
                    conversationID: activeID,
                    senderAgentID: ownerAgentID,
                    kind: kind,
                    bodyMarkdown: body,
                    expectsReply: kind == .question
                )
                if kind == .question {
                    _ = try? await controller.refreshThread(
                        conversationID: logicalConversationID,
                        waitSeconds: 5
                    )
                }
            } catch {
                sendMessage = "The message could not be saved. Try again."
            }
        }
    }

    private func pollVisibleConversation() async {
        while !Task.isCancelled {
            do {
                _ = try await controller.refreshThread(
                    conversationID: logicalConversationID,
                    waitSeconds: 25
                )
                await markVisibleMessagesRead()
            } catch {
                do {
                    try await Task.sleep(for: .seconds(2))
                } catch {
                    return
                }
            }
        }
    }

    private func markVisibleMessagesRead() async {
        guard canWriteMessages,
              let lastCanonical = messages.last(where: { $0.sequence > 0 }),
              let physicalConversation = controller.conversations.first(where: {
                  $0.conversationID == lastCanonical.conversationID
              }),
              lastCanonical.sequence > physicalConversation.lastReadSeq
        else { return }
        _ = try? await controller.markRead(through: lastCanonical)
    }

    private var bottomID: String { "messaging-thread-bottom-\(logicalConversationID)" }

    private func scrollID(for message: MessagingMessageRecord) -> String {
        if message.sequence > 0 {
            return "messaging-message-scroll-\(message.conversationID)-\(message.sequence)"
        }
        return "messaging-outbox-scroll-\(message.conversationID)-\(message.clientKey ?? message.storageKey)"
    }

    private func focusThread(using proxy: ScrollViewProxy) {
        Task { @MainActor in
            await Task.yield()
            if let focusedSequence,
               let target = messages.first(where: {
                   $0.conversationID == (focusedConversationID ?? logicalConversationID)
                       && $0.sequence == focusedSequence
               })
            {
                scroll(
                    using: proxy,
                    to: scrollID(for: target),
                    anchor: .center
                )
            } else {
                scroll(using: proxy, to: bottomID, anchor: .bottom)
            }
        }
    }

    private func scroll(
        using proxy: ScrollViewProxy,
        to id: String,
        anchor: UnitPoint
    ) {
        if reduceMotion {
            proxy.scrollTo(id, anchor: anchor)
        } else {
            withAnimation(.easeInOut(duration: 0.2)) {
                proxy.scrollTo(id, anchor: anchor)
            }
        }
    }
}

@MainActor
private struct MessagingBubble: View {
    let message: MessagingMessageRecord
    let isOwner: Bool
    let senderName: String

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            if isOwner { Spacer(minLength: 38) }

            VStack(alignment: .leading, spacing: 6) {
                Text(senderName)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(isOwner ? StraylightTheme.signal : .secondary)
                    .accessibilityIdentifier(senderMarkerIdentifier)

                SafeMarkdownText(markdown: message.bodyMarkdown)
                    .font(.body)
                    .foregroundStyle(StraylightTheme.ink)
                    .fixedSize(horizontal: false, vertical: true)

                if !message.refs.isEmpty {
                    MessagingReferenceList(refs: message.refs)
                }

                HStack(spacing: 7) {
                    if message.kind == "question" {
                        Label("Question", systemImage: "questionmark.bubble")
                            .foregroundStyle(StraylightTheme.signal)
                    }
                    if let inReplyTo = message.inReplyTo {
                        Label("Reply to #\(inReplyTo)", systemImage: "arrowshape.turn.up.left")
                            .foregroundStyle(.secondary)
                    }
                    Text(DisplayDate.metadata(message.createdAt))
                        .foregroundStyle(.secondary)
                    deliveryLabel
                }
                .font(.caption2.weight(.medium))
            }
            .padding(11)
            .background(bubbleBackground, in: RoundedRectangle(cornerRadius: 8))
            .overlay {
                RoundedRectangle(cornerRadius: 8)
                    .stroke(bubbleBorder, lineWidth: 1)
            }
            .frame(maxWidth: 560, alignment: isOwner ? .trailing : .leading)

            if !isOwner { Spacer(minLength: 38) }
        }
        .frame(maxWidth: .infinity, alignment: isOwner ? .trailing : .leading)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(rowIdentifier)
    }

    @ViewBuilder
    private var deliveryLabel: some View {
        switch message.deliveryState {
        case .sending:
            Label("Sending", systemImage: "arrow.up.circle")
                .foregroundStyle(StraylightTheme.signal)
        case .queued:
            Label("Queued to retry", systemImage: "clock.arrow.circlepath")
                .foregroundStyle(StraylightTheme.amber)
        case .canonical:
            EmptyView()
        }
    }

    private var bubbleBackground: Color {
        if message.fromAgentID == nil { return StraylightTheme.canvas }
        return isOwner ? StraylightTheme.signal.opacity(0.12) : Color(uiColor: .systemBackground)
    }

    private var bubbleBorder: Color {
        isOwner ? StraylightTheme.signal.opacity(0.28) : StraylightTheme.line
    }

    private var rowIdentifier: String {
        if message.deliveryState == .queued {
            return "messaging-outbox-queued-\(message.clientKey ?? message.storageKey)"
        }
        if message.deliveryState == .sending {
            return "messaging-outbox-sending-\(message.clientKey ?? message.storageKey)"
        }
        return "messaging-message-\(message.conversationID)-\(message.sequence)"
    }

    private var senderMarkerIdentifier: String {
        guard message.sequence > 0, let fromAgentID = message.fromAgentID else {
            return "messaging-message-sender-pending"
        }
        return "messaging-message-from-\(fromAgentID)-\(message.conversationID)-\(message.sequence)"
    }
}

@MainActor
private struct MessagingReferenceList: View {
    let refs: [MessagingReference]

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(Array(refs.enumerated()), id: \.offset) { _, reference in
                if let rawURL = reference.url,
                   let url = URL(string: rawURL),
                   ["https", "http"].contains(url.scheme?.lowercased())
                {
                    Link(destination: url) {
                        Label(reference.label ?? url.host ?? "Open reference", systemImage: "link")
                    }
                } else if let entryReference = reference.entryReference {
                    if let link = WorkspaceEntryLink(
                        target: entryReference,
                        label: reference.label
                    ) {
                        NavigationLink {
                            ContextSourceView(
                                request: WorkspaceEntryRequest(link: link, sourcePath: "")
                            )
                        } label: {
                            Label(
                                reference.label ?? entryReference,
                                systemImage: "doc.text"
                            )
                        }
                        .accessibilityIdentifier(
                            "messaging-entry-reference-\(entryReference)"
                        )
                    }
                }
            }
        }
        .font(.caption)
        .foregroundStyle(StraylightTheme.signal)
    }
}
