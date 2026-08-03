import SwiftUI

struct RootView: View {
    @EnvironmentObject private var model: AppModel

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
                    Label("Straylight is unavailable", systemImage: "exclamationmark.icloud")
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

                Text("Straylight")
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(.white)

                ProgressView()
                    .tint(.white.opacity(0.9))
                    .accessibilityLabel("Opening Straylight")
            }
        }
        .accessibilityIdentifier("straylight-startup")
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
                NewsView()
            }
            .tabItem { Label("News", systemImage: "newspaper") }
            .tag(AppTab.news)

            NavigationStack {
                ArchiveView()
            }
            .tabItem { Label("Archive", systemImage: "calendar") }
            .tag(AppTab.archive)

            NavigationStack {
                MoreView()
            }
            .tabItem { Label("More", systemImage: "ellipsis") }
            .tag(AppTab.more)
        }
        .tint(StraylightTheme.signalBlue)
    }
}
