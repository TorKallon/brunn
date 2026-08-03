import SwiftUI

struct AlertsView: View {
    @EnvironmentObject private var model: AppModel
    @State private var filter: AlertFilter = .all

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Durable inbox")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(StraylightTheme.signal)
                        .textCase(.uppercase)
                    Text("Alerts")
                        .font(.largeTitle.bold())
                    Text("Briefings, material news, corrections, and operational items surfaced for you.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                .padding(.bottom, 16)

                Picker("Alert filter", selection: $filter) {
                    ForEach(AlertFilter.allCases) { value in
                        Text(value.title).tag(value)
                    }
                }
                .pickerStyle(.segmented)
                .padding(.bottom, 18)

                if let message = model.notificationMessage {
                    Label(message, systemImage: "exclamationmark.triangle")
                        .font(.footnote)
                        .foregroundStyle(StraylightTheme.amber)
                        .padding(.bottom, 12)
                }

                if visibleItems.isEmpty {
                    ContentUnavailableView {
                        Label(emptyTitle, systemImage: "bell")
                    } description: {
                        Text("A push failure never removes a durable alert from this inbox.")
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 48)
                } else {
                    VStack(spacing: 0) {
                        ForEach(visibleItems) { item in
                            Button {
                                Task {
                                    await model.openNotification(reference: item.notificationRef)
                                }
                            } label: {
                                AlertRow(notification: item)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Open alert")
                            .accessibilityIdentifier("alert-item-\(rawID(item.notificationRef))")

                            if item.id != visibleItems.last?.id {
                                Divider()
                            }
                        }
                    }
                    .background(.background)
                    .overlay {
                        RoundedRectangle(cornerRadius: 8)
                            .stroke(StraylightTheme.line, lineWidth: 1)
                    }
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                }

                if model.isLoadingMoreNotifications {
                    ProgressView("Loading older alerts…")
                        .frame(maxWidth: .infinity)
                        .padding(.top, 18)
                } else if !model.isDemo, model.canLoadMoreNotifications {
                    Button("Load older alerts") {
                        Task { await model.loadMoreNotifications() }
                    }
                    .frame(maxWidth: .infinity, minHeight: 44)
                    .padding(.top, 10)
                }

                Text("Push is an attention signal. Private detail is loaded here only after authenticated open.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.top, 14)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("alerts-list")
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("Alerts")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) { BrandMark() }
            if model.isRefreshingNotifications {
                ToolbarItem(placement: .topBarTrailing) { ProgressView() }
            }
        }
        .refreshable {
            await model.refreshNotifications()
        }
        .task {
            if model.notifications.isEmpty {
                await model.refreshNotifications()
            }
        }
        .navigationDestination(
            isPresented: Binding(
                get: { model.presentedNotification != nil },
                set: { isPresented in
                    if !isPresented { model.presentedNotification = nil }
                }
            )
        ) {
            if let notification = model.presentedNotification {
                AlertDetailView(notification: notification)
            }
        }
    }

    private var visibleItems: [StraylightNotification] {
        switch filter {
        case .all:
            model.notifications
        case .important:
            model.notifications.filter { $0.importance == .important }
        case .unread:
            model.notifications.filter(\.isUnread)
        }
    }

    private var emptyTitle: String {
        switch filter {
        case .all: "No alerts yet"
        case .important: "No important alerts"
        case .unread: "You’re caught up"
        }
    }

    private func rawID(_ reference: String) -> String {
        PushReference.rawNotificationID(reference) ?? reference
    }
}

private enum AlertFilter: String, CaseIterable, Identifiable {
    case all
    case important
    case unread

    var id: String { rawValue }
    var title: String { rawValue.capitalized }
}

private struct AlertRow: View {
    let notification: StraylightNotification

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 7) {
                AlertKindPill(kind: notification.kind)
                if notification.importance == .important {
                    Text("IMPORTANT")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(StraylightTheme.amber)
                }
                Spacer(minLength: 4)
                if notification.isUnread {
                    Circle()
                        .fill(StraylightTheme.signal)
                        .frame(width: 7, height: 7)
                        .accessibilityLabel("Unread")
                }
            }

            Text(notification.title)
                .font(.headline)
                .foregroundStyle(StraylightTheme.ink)
                .multilineTextAlignment(.leading)

            SafeMarkdownText(markdown: notification.body)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .lineLimit(3)

            Text(AlertDate.metadata(notification.occurredAt))
                .font(.caption2)
                .foregroundStyle(.tertiary)
        }
        .padding(13)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }
}

private struct AlertDetailView: View {
    @EnvironmentObject private var model: AppModel
    let notification: StraylightNotification

    private var current: StraylightNotification {
        model.presentedNotification?.notificationRef == notification.notificationRef
            ? model.presentedNotification ?? notification
            : notification
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        AlertKindPill(kind: current.kind)
                        if current.importance == .important {
                            Text("IMPORTANT")
                                .font(.caption.weight(.bold))
                                .foregroundStyle(StraylightTheme.amber)
                        }
                    }
                    Text(current.title)
                        .font(.title2.bold())
                        .foregroundStyle(StraylightTheme.ink)
                        .textSelection(.enabled)
                    Text(AlertDate.metadata(current.occurredAt))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }

                SafeMarkdownText(markdown: current.body)
                    .font(.body)
                    .textSelection(.enabled)

                targetAction

                if let source = current.source {
                    VStack(alignment: .leading, spacing: 7) {
                        Text("Pinned source")
                            .font(.headline)
                        Text(source.reference)
                            .font(.system(.caption, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                        if let versionRef = source.versionRef {
                            Text(versionRef)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .detailCard()
                }

                if !current.deliveries.isEmpty {
                    VStack(alignment: .leading, spacing: 9) {
                        Text("Delivery trace")
                            .font(.headline)
                        ForEach(current.deliveries) { delivery in
                            DeliveryRow(delivery: delivery)
                        }
                        Text("APNs acceptance is not proof that iOS displayed the alert.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    .detailCard()
                }

                Button {
                    Task { await model.acknowledgeNotification(current.notificationRef) }
                } label: {
                    Label(
                        current.acknowledgedAt == nil ? "Acknowledge" : "Acknowledged",
                        systemImage: "checkmark.circle"
                    )
                    .frame(maxWidth: .infinity, minHeight: 44)
                }
                .buttonStyle(.bordered)
                .disabled(current.acknowledgedAt != nil)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 18)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier(
                "alert-detail-\(PushReference.rawNotificationID(current.notificationRef) ?? current.notificationRef)"
            )
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("Alert detail")
        .navigationBarTitleDisplayMode(.inline)
    }

    @ViewBuilder
    private var targetAction: some View {
        switch current.target.type {
        case .notification:
            EmptyView()
        case .today, .briefing:
            Button {
                Task { await model.openNotificationTarget(current) }
            } label: {
                Label(
                    current.target.type == .briefing ? "Open exact briefing" : "Open Today",
                    systemImage: current.target.type == .briefing ? "sunrise" : "calendar"
                )
                .frame(maxWidth: .infinity, minHeight: 44)
            }
            .buttonStyle(.borderedProminent)
            .accessibilityIdentifier("alert-target-action")
        case .entry:
            NavigationLink {
                AlertSourceView(notification: current)
            } label: {
                Label("Open exact source", systemImage: "doc.text.magnifyingglass")
                    .frame(maxWidth: .infinity, minHeight: 44)
            }
            .buttonStyle(.borderedProminent)
            .accessibilityIdentifier("alert-target-action")
        }
    }
}

private struct AlertSourceView: View {
    @EnvironmentObject private var model: AppModel
    let notification: StraylightNotification
    @State private var item: WorkspaceReadItem?
    @State private var errorMessage: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Exact source")
                    .font(.caption.weight(.bold))
                    .foregroundStyle(StraylightTheme.signal)
                    .textCase(.uppercase)
                Text(notification.title)
                    .font(.title.bold())

                if let item, let text = item.text {
                    if let version = item.version {
                        Text("Pinned v\(version)")
                            .font(.caption.weight(.bold))
                            .foregroundStyle(StraylightTheme.signal)
                    }
                    Text(text)
                        .font(.system(.body, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
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
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("Source")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            do {
                item = try await model.readNotificationEntry(notification)
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }
}

private struct DeliveryRow: View {
    let delivery: StraylightNotificationDelivery

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(deliveryLabel)
                .font(.subheadline.weight(.semibold))
            if let timestamp = delivery.acceptedAt ?? delivery.failedAt {
                Text(AlertDate.metadata(timestamp))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if let error = delivery.lastErrorCode {
                Text(error)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(StraylightTheme.red)
            }
        }
    }

    private var deliveryLabel: String {
        switch delivery.state {
        case .suppressed: "Push suppressed"
        case .acceptedByAPNs: "Accepted by APNs"
        case .failed: "Delivery failed"
        case .queued: "Queued"
        case .running: "Sending"
        case .expired: "Expired"
        }
    }
}

private struct AlertKindPill: View {
    let kind: StraylightNotificationKind

    var body: some View {
        Text(kind.label.uppercased())
            .font(.caption2.weight(.bold))
            .foregroundStyle(color)
            .padding(.horizontal, 6)
            .padding(.vertical, 3)
            .background(color.opacity(0.09), in: RoundedRectangle(cornerRadius: 5))
    }

    private var color: Color {
        switch kind {
        case .briefingReady, .newsAlert: StraylightTheme.signal
        case .correction: StraylightTheme.red
        case .operational: StraylightTheme.amber
        }
    }
}

private enum AlertDate {
    static func metadata(_ raw: String) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let date = formatter.date(from: raw) ?? {
            formatter.formatOptions = [.withInternetDateTime]
            return formatter.date(from: raw)
        }()
        return date?.formatted(date: .abbreviated, time: .shortened) ?? raw
    }
}

private extension View {
    func detailCard() -> some View {
        padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.background, in: RoundedRectangle(cornerRadius: 8))
            .overlay {
                RoundedRectangle(cornerRadius: 8)
                    .stroke(StraylightTheme.line, lineWidth: 1)
            }
    }
}
