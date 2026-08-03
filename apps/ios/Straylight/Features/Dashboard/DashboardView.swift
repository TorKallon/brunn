import Charts
import SwiftUI

struct DashboardView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 22) {
                NavigationLink {
                    SearchView()
                } label: {
                    DashboardSearchLink()
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("dashboard-search")

                if let message = model.dashboardMessage {
                    Label(message, systemImage: "chart.bar.xaxis")
                        .font(.footnote)
                        .foregroundStyle(StraylightTheme.amber)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(12)
                        .background(.background, in: RoundedRectangle(cornerRadius: 10))
                }

                if let dashboard = model.dashboard {
                    DashboardStorageSection(dashboard: dashboard)
                    DashboardActivitySection(dashboard: dashboard)
                    DashboardAccessSection(
                        clients: dashboard.access,
                        currentCredentialID: model.currentCredentialID
                    )
                } else if model.isRefreshingDashboard {
                    ProgressView("Loading workspace overview…")
                        .frame(maxWidth: .infinity, minHeight: 180)
                        .accessibilityIdentifier("dashboard-loading")
                } else {
                    BoundaryNotice(
                        symbol: "chart.bar.xaxis",
                        title: "Usage overview unavailable",
                        detail: "Pull to retry storage, activity, and access details."
                    )
                }
            }
            .frame(maxWidth: 760)
            .frame(maxWidth: .infinity)
            .padding(.horizontal, 14)
            .padding(.top, 14)
            .padding(.bottom, 36)
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("Home")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) { BrandMark() }
            if model.isRefreshingDashboard {
                ToolbarItem(placement: .topBarTrailing) {
                    ProgressView()
                        .accessibilityLabel("Refreshing workspace overview")
                }
            }
        }
        .refreshable {
            await model.refreshDashboard()
        }
        .task {
            await model.refreshDashboardIfNeeded()
        }
        .accessibilityIdentifier("dashboard-home")
    }
}

private struct DashboardSearchLink: View {
    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "magnifyingglass")
                .font(.headline)
                .foregroundStyle(StraylightTheme.signalBlue)
                .frame(width: 38, height: 38)
                .background(StraylightTheme.signalBlue.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
            VStack(alignment: .leading, spacing: 2) {
                Text("Search durable memory")
                    .font(.headline)
                    .foregroundStyle(.primary)
                Text("Find a source and open its exact stored version")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            Image(systemName: "chevron.right")
                .font(.caption.weight(.bold))
                .foregroundStyle(.tertiary)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background, in: RoundedRectangle(cornerRadius: 13))
        .overlay {
            RoundedRectangle(cornerRadius: 13)
                .stroke(StraylightTheme.line, lineWidth: 1)
        }
    }
}

private struct DashboardStorageSection: View {
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    let dashboard: WorkspaceDashboardData

    private var columns: [GridItem] {
        dynamicTypeSize.isAccessibilitySize
            ? [GridItem(.flexible())]
            : [GridItem(.flexible()), GridItem(.flexible())]
    }

    var body: some View {
        DashboardSectionHeader(
            eyebrow: "Storage",
            title: "What Straylight is holding",
            detail: "g\(dashboard.workspaceGeneration.formatted())"
        )
        LazyVGrid(columns: columns, spacing: 10) {
            StorageMetricCard(
                title: "Text artifacts",
                symbol: "doc.text",
                metric: dashboard.storage.text,
                tint: StraylightTheme.signalBlue,
                identifier: "dashboard-storage-text"
            )
            StorageMetricCard(
                title: "S3 object versions",
                symbol: "externaldrive",
                metric: dashboard.storage.binary,
                tint: StraylightTheme.signalCyan,
                identifier: "dashboard-storage-binary"
            )
        }
    }
}

private struct StorageMetricCard: View {
    let title: String
    let symbol: String
    let metric: DashboardStorageMetric
    let tint: Color
    let identifier: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Image(systemName: symbol)
                .font(.headline)
                .foregroundStyle(tint)
                .frame(width: 34, height: 34)
                .background(tint.opacity(0.1), in: RoundedRectangle(cornerRadius: 9))
            Text(title)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            Text(metric.count?.formatted() ?? "Unavailable")
                .font(.title2.bold())
                .contentTransition(.numericText())
            Text(metric.sizeBytes.map(DashboardFormat.bytes) ?? "Size unavailable")
                .font(.caption)
                .foregroundStyle(.secondary)
            if metric.status == "stale" {
                Text("Last observed inventory")
                    .font(.caption2)
                    .foregroundStyle(StraylightTheme.amber)
            }
        }
        .frame(maxWidth: .infinity, minHeight: 138, alignment: .leading)
        .padding(14)
        .background(.background, in: RoundedRectangle(cornerRadius: 13))
        .overlay {
            RoundedRectangle(cornerRadius: 13)
                .stroke(StraylightTheme.line, lineWidth: 1)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
        .accessibilityValue(accessibilityValue)
        .accessibilityIdentifier(identifier)
    }

    private var accessibilityValue: String {
        guard let count = metric.count, let size = metric.sizeBytes else {
            return "Inventory unavailable"
        }
        return "\(count.formatted()) items, \(DashboardFormat.bytes(size))"
    }
}

private struct DashboardActivitySection: View {
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    let dashboard: WorkspaceDashboardData

    private var columns: [GridItem] {
        dynamicTypeSize.isAccessibilitySize
            ? [GridItem(.flexible())]
            : [GridItem(.flexible()), GridItem(.flexible())]
    }

    var body: some View {
        DashboardSectionHeader(
            eyebrow: "Detailed Activity",
            title: "A rough pulse of daily usage",
            detail: dashboard.timezone
        )
        if let tracking = dashboard.tracking, tracking.status != "enabled" {
            Label(
                tracking.status == "disabled"
                    ? "Usage tracking is unavailable; today’s zeroes are not authoritative."
                    : "Usage tracking is degraded; recent totals may be incomplete.",
                systemImage: "exclamationmark.triangle"
            )
            .font(.footnote)
            .foregroundStyle(StraylightTheme.amber)
        }
        LazyVGrid(columns: columns, spacing: 10) {
            TodayActivityCard(
                title: "Reads today",
                value: activityAvailable ? dashboard.today.readOperations : nil,
                detail: activityAvailable
                    ? "\(DashboardFormat.bytes(dashboard.today.readBytes)) returned"
                    : "Tracking unavailable",
                tint: StraylightTheme.signalBlue
            )
            TodayActivityCard(
                title: "Writes today",
                value: activityAvailable ? dashboard.today.writeOperations : nil,
                detail: activityAvailable
                    ? "\(DashboardFormat.bytes(dashboard.today.writeBytes)) committed"
                    : "Tracking unavailable",
                tint: StraylightTheme.signalCyan
            )
        }
        DashboardUsageChart(
            title: "Operations",
            points: dashboard.activity,
            readValue: { Double($0.readOperations) },
            writeValue: { Double($0.writeOperations) },
            valueDescription: { Int64($0).formatted() },
            identifier: "dashboard-chart-operations"
        )
        DashboardUsageChart(
            title: "Data moved",
            points: dashboard.activity,
            readValue: { Double($0.readBytes) },
            writeValue: { Double($0.writeBytes) },
            valueDescription: { DashboardFormat.bytes(Int64($0)) },
            identifier: "dashboard-chart-bytes"
        )
        Text(activityFootnote)
            .font(.caption2)
            .foregroundStyle(.tertiary)
            .fixedSize(horizontal: false, vertical: true)
    }

    private var activityFootnote: String {
        var value = "Tracked content operations only; dashboard refreshes are excluded."
        if let started = dashboard.activityTrackingStartedAt {
            value += " Tracking began \(DisplayDate.metadata(started))."
        }
        return value
    }

    private var activityAvailable: Bool {
        dashboard.tracking?.status != "disabled"
    }
}

private struct TodayActivityCard: View {
    let title: String
    let value: Int64?
    let detail: String
    let tint: Color

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            Text(value?.formatted() ?? "Unavailable")
                .font(.title2.bold())
                .contentTransition(.numericText())
            Text(detail)
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, minHeight: 90, alignment: .leading)
        .padding(.leading, 15)
        .padding(.vertical, 12)
        .background(.background, in: RoundedRectangle(cornerRadius: 11))
        .overlay(alignment: .leading) {
            RoundedRectangle(cornerRadius: 2)
                .fill(tint)
                .frame(width: 3)
                .padding(.vertical, 10)
        }
    }
}

private struct DashboardUsageChart: View {
    let title: String
    let points: [DashboardActivityPoint]
    let readValue: (DashboardActivityPoint) -> Double
    let writeValue: (DashboardActivityPoint) -> Double
    let valueDescription: (Double) -> String
    let identifier: String

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text(title)
                    .font(.subheadline.weight(.semibold))
                Spacer()
                ChartLegend()
            }
            Chart {
                ForEach(points) { point in
                    BarMark(
                        x: .value("Day", DashboardDate.shortDay(point.date)),
                        y: .value("Value", readValue(point))
                    )
                    .foregroundStyle(by: .value("Operation", "Reads"))
                    .position(by: .value("Operation", "Reads"))

                    BarMark(
                        x: .value("Day", DashboardDate.shortDay(point.date)),
                        y: .value("Value", writeValue(point))
                    )
                    .foregroundStyle(by: .value("Operation", "Writes"))
                    .position(by: .value("Operation", "Writes"))
                }
            }
            .chartForegroundStyleScale([
                "Reads": StraylightTheme.signalBlue,
                "Writes": StraylightTheme.signalCyan,
            ])
            .chartLegend(.hidden)
            .chartYAxis {
                AxisMarks(position: .leading) { value in
                    AxisGridLine()
                    AxisValueLabel {
                        if let number = value.as(Double.self) {
                            Text(DashboardFormat.compact(number))
                        }
                    }
                }
            }
            .frame(height: 170)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(title)
            .accessibilityValue(accessibilitySummary)
        }
        .padding(14)
        .background(.background, in: RoundedRectangle(cornerRadius: 13))
        .overlay {
            RoundedRectangle(cornerRadius: 13)
                .stroke(StraylightTheme.line, lineWidth: 1)
        }
        .accessibilityIdentifier(identifier)
    }

    private var accessibilitySummary: String {
        points.map { point in
            "\(DisplayDate.metadata(point.date)): \(valueDescription(readValue(point))) read, \(valueDescription(writeValue(point))) written"
        }.joined(separator: "; ")
    }
}

private struct ChartLegend: View {
    var body: some View {
        HStack(spacing: 9) {
            legend("Reads", color: StraylightTheme.signalBlue)
            legend("Writes", color: StraylightTheme.signalCyan)
        }
        .font(.caption2)
        .foregroundStyle(.secondary)
        .accessibilityHidden(true)
    }

    private func legend(_ title: String, color: Color) -> some View {
        HStack(spacing: 4) {
            Circle().fill(color).frame(width: 6, height: 6)
            Text(title)
        }
    }
}

private struct DashboardAccessSection: View {
    let clients: [DashboardAccessClient]
    let currentCredentialID: String?

    var body: some View {
        DashboardSectionHeader(
            eyebrow: "Access",
            title: "Connected clients",
            detail: "\(clients.filter { $0.status == "active" }.count) active"
        )
        VStack(spacing: 0) {
            ForEach(Array(clients.enumerated()), id: \.element.id) { index, client in
                AccessClientRow(
                    client: client,
                    current: client.id == currentCredentialID
                )
                if index < clients.count - 1 {
                    Divider().padding(.leading, 52)
                }
            }
            if clients.isEmpty {
                Label("No connected clients are visible.", systemImage: "checkmark.shield")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 90)
            }
        }
        .background(.background, in: RoundedRectangle(cornerRadius: 13))
        .overlay {
            RoundedRectangle(cornerRadius: 13)
                .stroke(StraylightTheme.line, lineWidth: 1)
        }
        .accessibilityIdentifier("dashboard-access-list")
    }
}

private struct AccessClientRow: View {
    let client: DashboardAccessClient
    let current: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 11) {
            Image(systemName: client.kind == "web_ui" ? "safari" : "key.horizontal")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(client.status == "active" ? StraylightTheme.signalBlue : .secondary)
                .frame(width: 36, height: 36)
                .background(StraylightTheme.signalBlue.opacity(0.08), in: RoundedRectangle(cornerRadius: 9))

            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 6) {
                    Text(client.name)
                        .font(.subheadline.weight(.semibold))
                        .lineLimit(2)
                    if current {
                        Text("THIS CLIENT")
                            .font(.system(size: 9, weight: .bold))
                            .foregroundStyle(StraylightTheme.signalBlue)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 2)
                            .background(StraylightTheme.signalBlue.opacity(0.1), in: RoundedRectangle(cornerRadius: 4))
                    }
                }
                Text(lastActivity)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(identityDetail)
                    .font(.caption2.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                Text("\(client.access.replacingOccurrences(of: "_", with: " ").localizedCapitalized) · \(todayOperations)")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                if let lastOperation = client.lastOperation {
                    Text("Last operation · \(lastOperation)")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
            }
            Spacer(minLength: 5)
            StatusPill(
                text: client.status,
                color: client.status == "active" ? StraylightTheme.signalBlue : .secondary
            )
        }
        .padding(12)
        .opacity(client.status == "active" ? 1 : 0.64)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("dashboard-access-\(client.id)")
    }

    private var lastActivity: String {
        guard let value = client.lastUsedAt else { return "No usage recorded" }
        return "Last activity \(DisplayDate.metadata(value))"
    }

    private var identityDetail: String {
        let scope = client.scopeIDs.isEmpty ? "root scope" : client.scopeIDs.joined(separator: ", ")
        let capabilities = client.capabilities.isEmpty
            ? "capabilities not reported"
            : client.capabilities.joined(separator: ", ")
        let management = client.manageable ? "manageable" : "system principal"
        return "\(client.id) · \(scope) · \(capabilities) · \(management)"
    }

    private var todayOperations: String {
        let total = client.readOperationsToday + client.writeOperationsToday
        return total == 0 ? "no tracked activity today" : "\(total.formatted()) today"
    }
}

private struct DashboardSectionHeader: View {
    let eyebrow: String
    let title: String
    let detail: String

    var body: some View {
        HStack(alignment: .bottom) {
            VStack(alignment: .leading, spacing: 3) {
                Text(eyebrow.uppercased())
                    .font(.caption2.weight(.bold))
                    .tracking(0.7)
                    .foregroundStyle(StraylightTheme.signalBlue)
                Text(title)
                    .font(.headline)
            }
            Spacer(minLength: 8)
            Text(detail)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .padding(.top, 2)
    }
}

enum DashboardDate {
    static func dayKey(_ date: Date, timezone: TimeZone) -> String {
        let formatter = DateFormatter()
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timezone
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter.string(from: date)
    }

    static func shortDay(_ date: String, timezone: TimeZone = .current) -> String {
        let parser = DateFormatter()
        parser.calendar = Calendar(identifier: .gregorian)
        parser.locale = Locale(identifier: "en_US_POSIX")
        parser.timeZone = timezone
        parser.dateFormat = "yyyy-MM-dd"
        guard let parsed = parser.date(from: date) else { return String(date.suffix(5)) }
        let formatter = DateFormatter()
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timezone
        formatter.dateFormat = "EEE"
        return formatter.string(from: parsed)
    }
}

private enum DashboardFormat {
    static func bytes(_ value: Int64) -> String {
        ByteCountFormatter.string(fromByteCount: value, countStyle: .file)
    }

    static func compact(_ value: Double) -> String {
        value.formatted(.number.notation(.compactName).precision(.fractionLength(0 ... 1)))
    }
}
