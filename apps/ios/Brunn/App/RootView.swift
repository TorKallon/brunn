import SwiftUI

struct RootView: View {
    @Environment(\.scenePhase) private var scenePhase
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var locationReporter: LocationReporter
    @AppStorage(AppAppearance.storageKey) private var appearanceRawValue =
        AppAppearance.defaultValue.rawValue
    @AppStorage(LocationPermissionPromptPolicy.storageKey)
    private var locationPermissionPromptRevision = 0
    @AppStorage(LocationPermissionPromptPolicy.userStorageKey)
    private var locationPermissionPromptUserID = ""
    @State private var showingLocationPermissionPrimer = false

    var body: some View {
        Group {
            switch model.phase {
            case .launching:
                StartupView()
            case .connectionRequired:
                ConnectionView()
            case .ready:
                MainTabView()
            case let .failed(message):
                ContentUnavailableView {
                    Label("Brunn is unavailable", systemImage: "exclamationmark.icloud")
                } description: {
                    Text(message)
                } actions: {
                    Button("Try again") {
                        Task { await model.retryBootstrap() }
                    }
                    Button("Explore demo") {
                        model.enterDemo()
                    }
                }
            }
        }
        .preferredColorScheme(appearance.colorScheme)
        .sheet(
            isPresented: $showingLocationPermissionPrimer,
            onDismiss: markLocationPermissionPromptHandled
        ) {
            LocationPrimerView(onFinish: markLocationPermissionPromptHandled)
        }
        .onAppear(perform: evaluateLocationPermissionPrompt)
        .onChange(of: model.phase) { _, _ in
            evaluateLocationPermissionPrompt()
        }
        .onChange(of: model.connectionValidated) { _, _ in
            evaluateLocationPermissionPrompt()
        }
        .onChange(of: model.isDemo) { _, _ in
            evaluateLocationPermissionPrompt()
        }
        .onChange(of: model.user?.id) { _, _ in
            evaluateLocationPermissionPrompt()
        }
        .onChange(of: locationReporter.authorizationStatus) { _, _ in
            evaluateLocationPermissionPrompt()
        }
        .onChange(of: locationReporter.reportingEnabled) { _, _ in
            evaluateLocationPermissionPrompt()
        }
        .onChange(of: locationReporter.validatedCredentialUserID) { _, _ in
            evaluateLocationPermissionPrompt()
        }
        .onChange(of: scenePhase) { _, _ in
            evaluateLocationPermissionPrompt()
        }
    }

    private var appearance: AppAppearance {
        AppAppearance(rawValue: appearanceRawValue) ?? .defaultValue
    }

    private func evaluateLocationPermissionPrompt() {
        let decision = LocationPermissionPromptPolicy.decision(
            isReady: model.phase == .ready,
            connectionValidated: model.connectionValidated,
            isDemo: model.isDemo,
            userID: model.user?.id,
            sceneIsActive: scenePhase == .active,
            reportingEnabled: locationReporter.reportingEnabled,
            credentialBoundToUser: locationReporter.validatedCredentialUserID == model.user?.id,
            storedRevision: locationPermissionPromptRevision,
            storedUserID: locationPermissionPromptUserID,
            permissionState: LocationPermissionState(locationReporter.authorizationStatus)
        )
        switch decision {
        case .present:
            showingLocationPermissionPrimer = true
        case .markHandled:
            markLocationPermissionPromptHandled()
        case .none:
            break
        }
    }

    private func markLocationPermissionPromptHandled() {
        guard let userID = model.user?.id, !userID.isEmpty else { return }
        locationPermissionPromptUserID = userID
        locationPermissionPromptRevision = LocationPermissionPromptPolicy.handledRevision(
            storedRevision: locationPermissionPromptRevision
        )
    }
}

private struct StartupView: View {
    var body: some View {
        GeometryReader { geometry in
            Image("LaunchWaterline")
                .resizable()
                .scaledToFill()
                .frame(width: geometry.size.width, height: geometry.size.height)
                .clipped()
                .ignoresSafeArea()
        }
        .ignoresSafeArea()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Opening Brunn")
        .accessibilityIdentifier("brunn-startup")
    }
}

private struct MainTabView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        TabView(selection: $model.selectedTab) {
            NavigationStack {
                DashboardView()
            }
            .tabItem { Label("Home", systemImage: "square.grid.2x2") }
            .tag(AppTab.dashboard)

            NavigationStack {
                TodayView()
            }
            .tabItem { Label("Today", systemImage: "sunrise") }
            .tag(AppTab.today)

            NavigationStack {
                TasksView()
            }
            .tabItem { Label("Tasks", systemImage: "checklist") }
            .tag(AppTab.tasks)

            if model.messagingEnabled, let messagingController = model.messagingController {
                NavigationStack {
                    AgentsView(
                        controller: messagingController,
                        canWriteMessages: model.canWriteMessages,
                        focusedConversationID: model.focusedMessagingConversationID,
                        focusedSequence: model.focusedMessagingSequence,
                        onCreateConversation: { participants, subject in
                            try await model.createMessagingConversation(
                                participants: participants,
                                subject: subject
                            )
                        }
                    )
                }
                .tabItem { Label("Agents", systemImage: "bubble.left.and.bubble.right") }
                .badge(messagingUnreadCount(messagingController))
                .tag(AppTab.agents)
            }

            NavigationStack {
                AlertsView()
            }
            .tabItem { Label("Alerts", systemImage: "bell") }
            .badge(model.notificationUnreadCount)
            .tag(AppTab.alerts)

            NavigationStack {
                ArchiveView()
            }
            .tabItem { Label("Archive", systemImage: "calendar") }
            .tag(AppTab.archive)

            NavigationStack {
                MoreView()
            }
            .tabItem { Label("Settings", systemImage: "gearshape") }
            .tag(AppTab.more)
        }
        .tint(BrunnTheme.signalBlue)
    }

    private func messagingUnreadCount(_ controller: MessagingController) -> Int {
        let count = controller.conversations.reduce(Int64(0)) { partial, conversation in
            partial + max(conversation.unreadCount, 0)
        }
        return count > 0 ? Int(min(count, Int64(Int.max))) : 0
    }
}
