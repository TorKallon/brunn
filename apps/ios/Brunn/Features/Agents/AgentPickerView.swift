import SwiftUI

@MainActor
struct AgentPickerView: View {
    @Environment(\.dismiss) private var dismiss

    let agents: [MessagingAgentRecord]
    let onCreate: MessagingConversationCreator
    let onCreated: @MainActor (String) -> Void

    @State private var mode = ConversationMode.direct
    @State private var selectedAgentIDs = Set<String>()
    @State private var subject = ""
    @State private var isCreating = false
    @State private var createMessage: String?

    var body: some View {
        List {
            Section {
                Picker("Conversation type", selection: $mode) {
                    ForEach(ConversationMode.allCases) { value in
                        Text(value.label).tag(value)
                    }
                }
                .pickerStyle(.segmented)
                .accessibilityIdentifier("messaging-picker-mode")

                if mode == .group {
                    TextField("Group subject (optional)", text: $subject)
                        .textInputAutocapitalization(.sentences)
                        .accessibilityIdentifier("messaging-picker-subject")
                }
            } footer: {
                Text(mode == .direct
                    ? "Choose one registered agent."
                    : "Choose two or more registered agents. The owner is included automatically.")
            }

            Section("Active principals") {
                if selectableAgents.isEmpty {
                    Text("No active agent principals are available.")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(selectableAgents, id: \.agentID) { agent in
                        Button {
                            toggle(agent.agentID)
                        } label: {
                            HStack(spacing: 12) {
                                MessagingAgentPresenceRow(agent: agent)
                                Image(systemName: selectedAgentIDs.contains(agent.agentID)
                                    ? "checkmark.circle.fill"
                                    : "circle")
                                    .font(.title3)
                                    .foregroundStyle(selectedAgentIDs.contains(agent.agentID)
                                        ? BrunnTheme.signal
                                        : Color.secondary)
                                    .accessibilityHidden(true)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(
                            "\(agent.displayName), \(agent.online ? "online" : "offline")"
                        )
                        .accessibilityValue(
                            selectedAgentIDs.contains(agent.agentID) ? "Selected" : "Not selected"
                        )
                        .accessibilityIdentifier("messaging-picker-agent-\(agent.agentID)")
                    }
                }
            }

            if let createMessage {
                Section {
                    Label(createMessage, systemImage: "exclamationmark.triangle")
                        .font(.footnote)
                        .foregroundStyle(BrunnTheme.red)
                        .accessibilityIdentifier("messaging-picker-message")
                }
            }
        }
        .navigationTitle(mode == .direct ? "New conversation" : "New group")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button(isCreating ? "Creating…" : "Create") {
                    create()
                }
                .disabled(!canCreate || isCreating)
                .accessibilityIdentifier("messaging-picker-create")
            }
        }
        .onChange(of: mode) { _, newMode in
            createMessage = nil
            if newMode == .direct, selectedAgentIDs.count > 1,
               let first = selectedAgentIDs.sorted().first
            {
                selectedAgentIDs = [first]
            }
        }
        .accessibilityIdentifier("messaging-agent-picker")
    }

    private var selectableAgents: [MessagingAgentRecord] {
        agents.filter { !$0.archived && $0.principalKind != "owner" }
    }

    private var canCreate: Bool {
        switch mode {
        case .direct:
            selectedAgentIDs.count == 1
        case .group:
            selectedAgentIDs.count >= 2
        }
    }

    private func toggle(_ agentID: String) {
        createMessage = nil
        switch mode {
        case .direct:
            selectedAgentIDs = selectedAgentIDs == [agentID] ? [] : [agentID]
        case .group:
            if selectedAgentIDs.contains(agentID) {
                selectedAgentIDs.remove(agentID)
            } else {
                selectedAgentIDs.insert(agentID)
            }
        }
    }

    private func create() {
        guard canCreate else { return }
        let participants = selectedAgentIDs.sorted()
        let trimmedSubject = subject.trimmingCharacters(in: .whitespacesAndNewlines)
        isCreating = true
        createMessage = nil
        Task {
            do {
                let conversationID = try await onCreate(
                    participants,
                    trimmedSubject.isEmpty ? nil : trimmedSubject
                )
                isCreating = false
                onCreated(conversationID)
            } catch {
                isCreating = false
                createMessage = "The conversation could not be created. Try again."
            }
        }
    }
}

private enum ConversationMode: String, CaseIterable, Identifiable {
    case direct
    case group

    var id: String { rawValue }
    var label: String { rawValue.capitalized }
}
