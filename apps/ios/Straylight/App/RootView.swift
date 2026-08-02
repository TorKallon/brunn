import SwiftUI

struct RootView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Group {
            switch model.phase {
            case .launching:
                ProgressView("Opening Straylight…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(StraylightTheme.canvas)
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

private struct MainTabView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        TabView(selection: $model.selectedTab) {
            NavigationStack {
                TodayView()
            }
            .tabItem { Label("Today", systemImage: "house") }
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
    }
}
