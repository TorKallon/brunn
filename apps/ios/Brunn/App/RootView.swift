import SwiftUI

struct RootView: View {
    @EnvironmentObject private var model: AppModel
    @AppStorage(AppAppearance.storageKey) private var appearanceRawValue =
        AppAppearance.defaultValue.rawValue

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
    }

    private var appearance: AppAppearance {
        AppAppearance(rawValue: appearanceRawValue) ?? .defaultValue
    }
}

private struct StartupView: View {
    var body: some View {
        ZStack {
            Color("LaunchBackground")
                .ignoresSafeArea()

            VStack(spacing: 18) {
                Image("LaunchSignal")
                    .resizable()
                    .scaledToFit()
                    .frame(width: 132, height: 132)
                    .accessibilityHidden(true)

                Text("brunn")
                    .font(.title2.weight(.medium))
                    .fontDesign(.serif)
                    .tracking(-0.66)
                    .foregroundStyle(.white)

                ProgressView()
                    .tint(.white.opacity(0.9))
                    .accessibilityLabel("Opening Brunn")
            }
        }
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
