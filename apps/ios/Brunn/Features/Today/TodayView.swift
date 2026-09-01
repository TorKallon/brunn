import SwiftUI

struct TodayView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if let message = model.connectionMessage, !model.isDemo {
                        ConnectionBanner(message: message, isDemo: model.isDemo)
                    }

                    if let briefing = model.latestBriefing {
                        BriefingReader(
                            briefing: briefing,
                            cachedAt: model.cachedAt,
                            focusedItemID: model.focusedBriefingItemID
                        )
                        .id(briefing.entryRef)
                    } else {
                        BoundaryNotice(
                            symbol: "sunrise",
                            title: "No briefing is published yet",
                            detail: "When an agent publishes a structured briefing, it will appear here."
                        )
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 12)
                .padding(.top, 16)
                .padding(.bottom, 32)
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("today-scroll")
            .background(BrunnTheme.canvas)
            .navigationTitle("Today")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { BrandMark() }
            }
            .refreshable { await model.refreshBriefing() }
            .overlay {
                if model.isRefreshingBriefing {
                    ProgressView()
                        .padding(12)
                        .background(.regularMaterial, in: Circle())
                        .accessibilityLabel("Refreshing briefing")
                }
            }
            .onAppear { scrollToFocusedItem(using: proxy) }
            .onChange(of: model.focusedBriefingItemID) { _, _ in
                scrollToFocusedItem(using: proxy)
            }
        }
    }

    private func scrollToFocusedItem(using proxy: ScrollViewProxy) {
        guard let itemID = model.focusedBriefingItemID else { return }
        Task { @MainActor in
            await Task.yield()
            if reduceMotion {
                proxy.scrollTo(itemID, anchor: .top)
            } else {
                withAnimation(.easeInOut(duration: 0.2)) {
                    proxy.scrollTo(itemID, anchor: .top)
                }
            }
        }
    }
}

struct AgentTasksView: View {
    @EnvironmentObject private var model: AppModel
    @State private var moreTapCount = 0
    @State private var showsBoundedList = false
    @State private var snoozeCandidate: AgentTaskCandidate?
    @State private var snoozeDate = Date.now.addingTimeInterval(86_400)
    @State private var waitCandidate: AgentTaskCandidate?
    @State private var waitingOn = ""

    private var projection: AgentTaskTodayProjection {
        AgentTaskTodayProjection.bounded(
            urgent: model.urgentTasks,
            next: model.nextTasks,
            contextsAvailable: model.selectedTaskContexts,
            baseLimit: 5 + (moreTapCount * 5),
            pinAllowance: 2
        )
    }

    private var matchingFetchedTasks: [AgentTaskCandidate] {
        (model.urgentTasks + model.nextTasks)
            .uniqued(by: \.taskRef)
            .filter { Set($0.requiredContexts).isSubset(of: model.selectedTaskContexts) }
    }

    private var remainingReadyCount: Int {
        model.taskNextRemaining + max(matchingFetchedTasks.count - projection.all.count, 0)
    }

    private var boundedListTasks: [AgentTaskCandidate] {
        Array(matchingFetchedTasks.prefix(25))
    }

    var body: some View {
        ScrollViewReader { _ in
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if let message = model.connectionMessage, !model.isDemo {
                        ConnectionBanner(message: message, isDemo: model.isDemo)
                    }

                    AgentTaskSurface(
                        projection: projection,
                        doneToday: model.doneToday,
                        projects: model.taskProjects,
                        todoistStatus: model.todoistStatus,
                        canWrite: model.canWriteTasks,
                        canRequestWriteAccess: !model.isDemo && model.connectionValidated,
                        hasStoredWriteAccess: model.hasStoredDeviceTaskCredential,
                        isRefreshing: model.isRefreshingTasks,
                        isConfiguringWriteAccess: model.isConfiguringDeviceTaskAccess,
                        mutatingRefs: model.mutatingTaskRefs,
                        message: model.taskMessage,
                        deviceAccessMessage: model.deviceTaskAccessMessage,
                        moreTapCount: moreTapCount,
                        nextRemaining: remainingReadyCount,
                        requestWriteAccess: {
                            Task { await model.bootstrapDeviceTaskAccess() }
                        },
                        complete: complete,
                        open: { candidate in
                            Task { await model.openTask(reference: candidate.taskRef) }
                        },
                        action: perform,
                        pickSnooze: { candidate in
                            snoozeCandidate = candidate
                            snoozeDate = .now.addingTimeInterval(86_400)
                        },
                        waitOn: { candidate in
                            waitCandidate = candidate
                            waitingOn = ""
                        },
                        more: showMore,
                        openProject: { project in
                            Task { await model.loadProject(project) }
                        }
                    )

                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 12)
                .padding(.top, 16)
                    .padding(.bottom, 32)
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("tasks-scroll")
            .background(BrunnTheme.canvas)
            .navigationTitle("Tasks")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    BrandMark()
                }
            }
            .refreshable {
                await model.refreshTaskSurface()
            }
            .overlay {
                if model.isRefreshingTasks {
                    ProgressView()
                        .padding(12)
                        .background(.regularMaterial, in: Circle())
                        .accessibilityLabel("Refreshing tasks")
                }
            }
            .sheet(item: $model.presentedTask) { task in
                AgentTaskDetailView(task: task)
                    .environmentObject(model)
            }
            .sheet(isPresented: $showsBoundedList) {
                NavigationStack {
                    AgentTaskBoundedList(
                        tasks: boundedListTasks,
                        canWrite: model.canWriteTasks,
                        complete: complete,
                        open: { candidate in
                            showsBoundedList = false
                            Task { await model.openTask(reference: candidate.taskRef) }
                        }
                    )
                }
            }
            .sheet(item: $snoozeCandidate) { candidate in
                NavigationStack {
                    Form {
                        DatePicker(
                            "Ready again",
                            selection: $snoozeDate,
                            in: Date.now...,
                            displayedComponents: [.date, .hourAndMinute]
                        )
                    }
                    .navigationTitle("Snooze task")
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("Cancel") { snoozeCandidate = nil }
                        }
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Snooze") {
                                let formatter = ISO8601DateFormatter()
                                Task {
                                    _ = await model.performTaskAction(
                                        candidate,
                                        operation: .snoozeUntil(formatter.string(from: snoozeDate))
                                    )
                                    snoozeCandidate = nil
                                }
                            }
                        }
                    }
                }
                .presentationDetents([.medium])
            }
            .sheet(item: $waitCandidate) { candidate in
                NavigationStack {
                    Form {
                        TextField("Who or what are you waiting on?", text: $waitingOn)
                            .accessibilityIdentifier("task-wait-on-input")
                    }
                    .navigationTitle("Wait on")
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("Cancel") { waitCandidate = nil }
                        }
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Save") {
                                Task {
                                    _ = await model.performTaskAction(
                                        candidate,
                                        operation: .waitOn(waitingOn.trimmingCharacters(in: .whitespacesAndNewlines))
                                    )
                                    waitCandidate = nil
                                }
                            }
                            .disabled(waitingOn.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        }
                    }
                }
                .presentationDetents([.medium])
            }
            .sheet(
                isPresented: Binding(
                    get: { model.selectedProjectState != nil },
                    set: { if !$0 { model.selectedProjectState = nil } }
                )
            ) {
                if let state = model.selectedProjectState {
                    NavigationStack {
                        AgentTaskProjectDetailView(state: state)
                            .environmentObject(model)
                    }
                }
            }
        }
    }

    private func complete(_ candidate: AgentTaskCandidate) {
        Task {
            if await model.performTaskAction(candidate, operation: .complete) {
                UINotificationFeedbackGenerator().notificationOccurred(.success)
            }
        }
    }

    private func perform(_ candidate: AgentTaskCandidate, _ operation: AgentTaskUpdateOperation) {
        Task { _ = await model.performTaskAction(candidate, operation: operation) }
    }

    private func showMore() {
        if moreTapCount < 2 {
            moreTapCount += 1
        } else {
            showsBoundedList = true
        }
    }

}

private extension Array {
    func uniqued<Key: Hashable>(by keyPath: KeyPath<Element, Key>) -> [Element] {
        var seen = Set<Key>()
        return filter { seen.insert($0[keyPath: keyPath]).inserted }
    }
}

private struct AgentTaskSurface: View {
    let projection: AgentTaskTodayProjection
    let doneToday: AgentTaskDoneSummaryData?
    let projects: [AgentTaskProject]
    let todoistStatus: AgentTaskTodoistStatus?
    let canWrite: Bool
    let canRequestWriteAccess: Bool
    let hasStoredWriteAccess: Bool
    let isRefreshing: Bool
    let isConfiguringWriteAccess: Bool
    let mutatingRefs: Set<String>
    let message: String?
    let deviceAccessMessage: String?
    let moreTapCount: Int
    let nextRemaining: Int
    let requestWriteAccess: () -> Void
    let complete: (AgentTaskCandidate) -> Void
    let open: (AgentTaskCandidate) -> Void
    let action: (AgentTaskCandidate, AgentTaskUpdateOperation) -> Void
    let pickSnooze: (AgentTaskCandidate) -> Void
    let waitOn: (AgentTaskCandidate) -> Void
    let more: () -> Void
    let openProject: (AgentTaskProject) -> Void

    @State private var doneExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if !canWrite {
                VStack(alignment: .leading, spacing: 9) {
                    Label("Task actions are locked on this iPhone", systemImage: "lock")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(BrunnTheme.amber)
                    Text("Enable secure device access to complete, snooze, or correct tasks.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                    if canRequestWriteAccess {
                        Button(action: requestWriteAccess) {
                            HStack {
                                if isConfiguringWriteAccess {
                                    ProgressView().controlSize(.small)
                                }
                                Text(
                                    isConfiguringWriteAccess
                                        ? "Enabling…"
                                        : hasStoredWriteAccess
                                            ? "Repair task actions"
                                            : "Enable task actions"
                                )
                            }
                            .frame(maxWidth: .infinity, minHeight: 44)
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(isConfiguringWriteAccess)
                        .accessibilityIdentifier("task-enable-actions")
                    } else {
                        Text("Connect this iPhone to enable task actions.")
                            .font(.footnote.weight(.medium))
                            .foregroundStyle(.secondary)
                    }
                    if let deviceAccessMessage {
                        Text(deviceAccessMessage)
                            .font(.footnote)
                            .foregroundStyle(BrunnTheme.amber)
                            .accessibilityIdentifier("task-device-access-message")
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.background, in: RoundedRectangle(cornerRadius: 8))
                .overlay { RoundedRectangle(cornerRadius: 8).stroke(BrunnTheme.line, lineWidth: 1) }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("task-view-only")
            }

            if let message {
                Label(message, systemImage: "exclamationmark.triangle")
                    .font(.footnote)
                    .foregroundStyle(BrunnTheme.amber)
                    .accessibilityIdentifier("task-message")
            }

            AgentTaskTodoistStatusCard(status: todoistStatus)

            if !projection.urgent.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    TaskSectionHeader(
                        title: "URGENT",
                        count: projection.urgent.count,
                        tint: BrunnTheme.red
                    )
                    ForEach(projection.urgent) { candidate in
                        actionRow(candidate)
                    }
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("task-urgent")
            }

            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    TaskSectionHeader(
                        title: "NEXT 5",
                        count: projection.next.count,
                        tint: BrunnTheme.signal
                    )
                    Spacer()
                    if isRefreshing {
                        ProgressView()
                            .controlSize(.small)
                            .accessibilityLabel("Refreshing tasks")
                    }
                }

                if projection.next.isEmpty {
                    Text("No ready tasks match the current availability filter.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, minHeight: 56, alignment: .leading)
                        .accessibilityIdentifier("task-next-empty")
                } else {
                    ForEach(projection.next) { candidate in
                        actionRow(candidate)
                    }
                }

                if nextRemaining > 0 || moreTapCount > 0 {
                    Button(action: more) {
                        Label(
                            moreTapCount < 2 ? "5 more" : "Open bounded list",
                            systemImage: moreTapCount < 2 ? "plus" : "list.bullet"
                        )
                        .frame(maxWidth: .infinity, minHeight: 44)
                    }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("task-five-more")
                }

                Text("\(max(nextRemaining, 0)) more ready · \(nextRemaining == 0 ? "Today is clear" : "backlog stays hidden")")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("task-remaining-count")
            }
            .padding(12)
            .background(.background, in: RoundedRectangle(cornerRadius: 8))
            .overlay {
                RoundedRectangle(cornerRadius: 8)
                    .stroke(BrunnTheme.line, lineWidth: 1)
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("task-next-card")

            if let doneToday {
                VStack(alignment: .leading, spacing: 8) {
                    Button {
                        doneExpanded.toggle()
                    } label: {
                        HStack {
                            Label("Done today", systemImage: "checkmark.circle.fill")
                                .font(.headline)
                                .foregroundStyle(BrunnTheme.success)
                            Spacer()
                            Text("\(doneToday.doneTodayCount)")
                                .font(.headline.monospacedDigit())
                                .foregroundStyle(BrunnTheme.success)
                            Image(systemName: "chevron.down")
                                .font(.caption.bold())
                                .rotationEffect(.degrees(doneExpanded ? 180 : 0))
                                .accessibilityHidden(true)
                        }
                    }
                    .buttonStyle(.plain)
                    .frame(minHeight: 44)
                    .accessibilityIdentifier("task-done-today")

                    if doneExpanded {
                        ForEach(doneToday.items.prefix(25)) { item in
                            HStack(alignment: .firstTextBaseline, spacing: 9) {
                                Image(systemName: "checkmark")
                                    .foregroundStyle(BrunnTheme.success)
                                    .accessibilityHidden(true)
                                Text(item.title)
                                    .font(.subheadline)
                                    .foregroundStyle(BrunnTheme.ink)
                                Spacer(minLength: 0)
                            }
                            .accessibilityElement(children: .combine)
                        }
                    }
                }
                .padding(12)
                .background(BrunnTheme.success.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
                .overlay {
                    RoundedRectangle(cornerRadius: 8)
                        .stroke(BrunnTheme.success.opacity(0.30), lineWidth: 1)
                }
            }

            if !projects.isEmpty {
                VStack(alignment: .leading, spacing: 9) {
                    TaskSectionHeader(
                        title: "PROJECTS",
                        count: projects.count,
                        tint: BrunnTheme.pulse
                    )
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 10) {
                            ForEach(projects.prefix(10)) { project in
                                Button {
                                    openProject(project)
                                } label: {
                                    VStack(alignment: .leading, spacing: 7) {
                                        Text(project.title)
                                            .font(.headline)
                                            .foregroundStyle(BrunnTheme.ink)
                                            .lineLimit(2)
                                        Text("\(project.openTaskCount) open")
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                        StatusPill(
                                            text: project.interest.uppercased(),
                                            color: project.interest == "hot" ? BrunnTheme.amber : BrunnTheme.pulse
                                        )
                                    }
                                    .padding(12)
                                    .frame(width: 172, alignment: .leading)
                                    .frame(minHeight: 112, alignment: .leading)
                                    .background(.background, in: RoundedRectangle(cornerRadius: 8))
                                    .overlay {
                                        RoundedRectangle(cornerRadius: 8)
                                            .stroke(BrunnTheme.line, lineWidth: 1)
                                    }
                                }
                                .buttonStyle(.plain)
                                .accessibilityIdentifier("task-project-\(project.slug)")
                            }
                        }
                    }
                }
                .accessibilityIdentifier("task-projects")
            }

        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("agent-task-surface")
    }

    @ViewBuilder
    private func actionRow(_ candidate: AgentTaskCandidate) -> some View {
        let row = AgentTaskCandidateRow(
            candidate: candidate,
            canWrite: canWrite,
            isMutating: mutatingRefs.contains(candidate.taskRef),
            complete: { complete(candidate) },
            open: { open(candidate) }
        )
        if canWrite {
            row.contextMenu {
                Button("Tomorrow", systemImage: "sunrise") {
                    action(candidate, .snooze(days: 1))
                }
                Button("3 days", systemImage: "calendar.badge.clock") {
                    action(candidate, .snooze(days: 3))
                }
                Button("Next week", systemImage: "calendar") {
                    action(candidate, .snooze(days: 7))
                }
                Button("Pick snooze date…", systemImage: "calendar.badge.plus") {
                    pickSnooze(candidate)
                }
                Button("Wait on…", systemImage: "hourglass") {
                    waitOn(candidate)
                }
                Button(
                    candidate.pinned ? "Unpin from Today" : "Pin to Today",
                    systemImage: candidate.pinned ? "pin.slash" : "pin"
                ) {
                    action(candidate, candidate.pinned ? .unpin : .pinToday)
                }
                if candidate.tier == 1 && candidate.hasInferredProvenance {
                    Button("Confirm hard deadline", systemImage: "checkmark.seal") {
                        action(candidate, .confirmHard)
                    }
                    Button("Make it a soft due date", systemImage: "calendar.badge.minus") {
                        action(candidate, .downgradeToSoft)
                    }
                }
            }
        } else {
            row
        }
    }
}

private struct AgentTaskTodoistStatusCard: View {
    let status: AgentTaskTodoistStatus?

    private var hasFailure: Bool {
        guard let status else { return false }
        if status.lastErrorCode != nil { return true }
        return ["error", "failed", "failure"].contains(status.lastOutcome?.lowercased())
    }

    private var isActive: Bool {
        status?.environmentEnabled == true
            && status?.tokenConfigured == true
            && status?.effectiveMode == "pull"
            && !hasFailure
    }

    private var title: String {
        guard let status else { return "Todoist status unavailable" }
        if !status.tokenConfigured { return "Todoist isn’t connected" }
        if !status.environmentEnabled || status.effectiveMode == "off" {
            return "Todoist import is off"
        }
        if hasFailure { return "Todoist import needs attention" }
        if status.effectiveMode == "import_once" { return "Todoist one-time import" }
        return "Todoist pull is active"
    }

    private var detail: String {
        guard let status else {
            return "Pull to refresh after the server connection is restored."
        }
        if !status.tokenConfigured {
            return "No Todoist tasks can import until a token is saved in Web Settings."
        }
        if !status.environmentEnabled {
            return "The deployment safety switch is off, so imports cannot run."
        }
        if status.effectiveMode == "off" {
            return "Import is saved as off. Change it in Web Settings when you want to pull tasks."
        }
        if hasFailure {
            if let code = status.lastErrorCode {
                return "The last pull failed with the content-free status code \(code)."
            }
            return "The last pull failed. Open Web Settings to review the integration."
        }
        if let outcome = status.lastOutcome {
            return "Last pull: \(outcome.replacingOccurrences(of: "_", with: " "))."
        }
        return "Waiting for the first pull."
    }

    private var tint: Color {
        hasFailure ? BrunnTheme.red : isActive ? BrunnTheme.signal : BrunnTheme.amber
    }

    private var symbol: String {
        hasFailure
            ? "exclamationmark.triangle"
            : isActive
                ? "arrow.triangle.2.circlepath.circle"
                : "pause.circle"
    }

    private var showsSettingsLink: Bool {
        guard let status else { return true }
        return hasFailure
            || !status.tokenConfigured
            || !status.environmentEnabled
            || status.effectiveMode == "off"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(title, systemImage: symbol)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(tint)
            Text(detail)
                .font(.footnote)
                .foregroundStyle(.secondary)

            if showsSettingsLink,
               let settingsURL = URL(string: "https://brunn.ai/settings")
            {
                Link(destination: settingsURL) {
                    Label("Open Web Settings", systemImage: "safari")
                        .frame(minHeight: 44)
                }
                .accessibilityIdentifier("task-todoist-settings")
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background, in: RoundedRectangle(cornerRadius: 8))
        .overlay { RoundedRectangle(cornerRadius: 8).stroke(BrunnTheme.line, lineWidth: 1) }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("task-todoist-status")
    }
}

private struct TaskSectionHeader: View {
    let title: String
    let count: Int
    let tint: Color

    var body: some View {
        HStack(spacing: 7) {
            Circle()
                .fill(tint)
                .frame(width: 7, height: 7)
                .accessibilityHidden(true)
            Text(title)
                .font(.caption.weight(.bold))
                .tracking(0.6)
                .foregroundStyle(tint)
            Text("\(count)")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
        }
    }
}

private struct AgentTaskCandidateRow: View {
    let candidate: AgentTaskCandidate
    let canWrite: Bool
    let isMutating: Bool
    let complete: () -> Void
    let open: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Button(action: complete) {
                Group {
                    if isMutating {
                        ProgressView().controlSize(.small)
                    } else {
                        Image(systemName: canWrite ? "circle" : "lock.circle")
                            .font(.title3)
                    }
                }
                .frame(width: 44, height: 44)
            }
            .buttonStyle(.plain)
            .foregroundStyle(canWrite ? BrunnTheme.success : .secondary)
            .disabled(!canWrite || isMutating)
            .accessibilityLabel(canWrite ? "Complete \(candidate.title)" : "View only")
            .accessibilityIdentifier("task-complete-\(candidate.taskRef)")

            Button(action: open) {
                VStack(alignment: .leading, spacing: 5) {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(candidate.title)
                            .font(.headline)
                            .foregroundStyle(BrunnTheme.ink)
                            .multilineTextAlignment(.leading)
                        if candidate.pinned {
                            Image(systemName: "pin.fill")
                                .font(.caption)
                                .foregroundStyle(BrunnTheme.pulse)
                                .accessibilityLabel("Pinned")
                        }
                        if candidate.hasInferredProvenance {
                            Image(systemName: "wand.and.stars")
                                .font(.caption)
                                .foregroundStyle(BrunnTheme.amber)
                                .accessibilityLabel("Inferred")
                        }
                        if candidate.provenanceMarkers.contains("todoist") {
                            Text("TODOIST")
                                .font(.caption2.weight(.bold))
                                .tracking(0.4)
                                .foregroundStyle(BrunnTheme.pulse)
                                .accessibilityLabel("Imported from Todoist")
                        }
                    }
                    if let project = candidate.project {
                        Text(project)
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(BrunnTheme.pulse)
                    }
                    Text(candidate.reason)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.leading)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(openAccessibilityLabel)
            .accessibilityIdentifier("task-row-\(candidate.taskRef)")
        }
        .padding(10)
        .background(.background, in: RoundedRectangle(cornerRadius: 8))
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .stroke(BrunnTheme.line, lineWidth: 1)
        }
    }

    private var openAccessibilityLabel: String {
        var parts = ["Open \(candidate.title).", candidate.reason]
        if candidate.provenanceMarkers.contains("todoist") {
            parts.append("Imported from Todoist.")
        }
        if candidate.pinned { parts.append("Pinned.") }
        if candidate.hasInferredProvenance { parts.append("Contains inferred task details.") }
        return parts.joined(separator: " ")
    }
}

private struct AgentTaskBoundedList: View {
    let tasks: [AgentTaskCandidate]
    let canWrite: Bool
    let complete: (AgentTaskCandidate) -> Void
    let open: (AgentTaskCandidate) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        List(tasks.prefix(25)) { candidate in
            AgentTaskCandidateRow(
                candidate: candidate,
                canWrite: canWrite,
                isMutating: false,
                complete: { complete(candidate) },
                open: { open(candidate) }
            )
        }
        .navigationTitle("Ready tasks")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .confirmationAction) {
                Button("Done") { dismiss() }
            }
        }
        .safeAreaInset(edge: .bottom) {
            Text("Bounded to 25 ready tasks · the full backlog is not shown on iPhone")
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(10)
                .frame(maxWidth: .infinity)
                .background(.regularMaterial)
        }
        .accessibilityIdentifier("task-bounded-list")
    }
}

private struct AgentTaskDetailView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let task: AgentTaskDetail
    @State private var correctedTitle: String

    init(task: AgentTaskDetail) {
        self.task = task
        _correctedTitle = State(initialValue: task.title)
    }

    private var candidate: AgentTaskCandidate {
        AgentTaskCandidate(
            taskRef: task.taskRef,
            entryRef: task.entryRef,
            version: task.version,
            title: task.title,
            status: task.status,
            project: task.task.project?.value,
            requiredContexts: task.task.requiredContexts?.value ?? [],
            tier: task.task.hardDue == nil ? 5 : 1,
            reason: task.task.hardDue == nil ? "Task detail" : "Hard deadline",
            provenanceMarkers: task.task.hardDue?.source == "owner" ? [] : [task.task.hardDue?.source ?? "derived"],
            pinned: task.task.todayPin != nil
        )
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    HStack {
                        StatusPill(
                            text: task.status.rawValue.uppercased(),
                            color: task.status == .done ? BrunnTheme.success : BrunnTheme.pulse
                        )
                        if task.task.hardDue?.source != nil,
                           task.task.hardDue?.source != "owner"
                        {
                            StatusPill(text: "INFERRED", color: BrunnTheme.amber, symbol: "wand.and.stars")
                        }
                    }

                    Text(task.title)
                        .font(.largeTitle.bold())
                        .foregroundStyle(BrunnTheme.ink)
                        .accessibilityIdentifier("task-detail-title")

                    if let notes = task.task.notes {
                        SafeMarkdownText(markdown: notes.value)
                            .font(.body)
                        SourceLine(source: notes.source)
                    }
                    if let project = task.task.project {
                        LabeledContent("Project", value: project.value)
                        SourceLine(source: project.source)
                    }
                    if let hardDue = task.task.hardDue {
                        LabeledContent("Hard deadline", value: hardDue.value)
                        SourceLine(source: hardDue.source)
                    }
                    if let contexts = task.task.requiredContexts, !contexts.value.isEmpty {
                        LabeledContent("Contexts", value: contexts.value.joined(separator: ", "))
                        SourceLine(source: contexts.source)
                    }

                    if model.canWriteTasks, task.status == .open || task.status == .waiting {
                        Button {
                            Task {
                                if await model.performTaskAction(candidate, operation: .complete) {
                                    UINotificationFeedbackGenerator().notificationOccurred(.success)
                                    dismiss()
                                }
                            }
                        } label: {
                            Label("Complete", systemImage: "checkmark.circle.fill")
                                .frame(maxWidth: .infinity, minHeight: 44)
                        }
                        .buttonStyle(.borderedProminent)
                        .tint(BrunnTheme.success)
                        .accessibilityIdentifier("task-detail-complete")

                        VStack(alignment: .leading, spacing: 8) {
                            Text("Correct title")
                                .font(.headline)
                            TextField("Task title", text: $correctedTitle)
                                .textFieldStyle(.roundedBorder)
                                .accessibilityIdentifier("task-correct-title")
                            Button("Save correction") {
                                Task {
                                    _ = await model.performTaskAction(
                                        candidate,
                                        operation: .correct(
                                            field: "title",
                                            value: .string(correctedTitle.trimmingCharacters(in: .whitespacesAndNewlines)),
                                            note: "Owner correction on iOS"
                                        )
                                    )
                                }
                            }
                            .disabled(
                                correctedTitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                    || correctedTitle == task.title
                            )
                            .accessibilityIdentifier("task-save-correction")
                        }
                        .padding(12)
                        .background(.background, in: RoundedRectangle(cornerRadius: 8))
                        .overlay {
                            RoundedRectangle(cornerRadius: 8)
                                .stroke(BrunnTheme.line, lineWidth: 1)
                        }

                        if task.task.hardDue?.source != nil,
                           task.task.hardDue?.source != "owner"
                        {
                            HStack {
                                Button("Confirm hard") {
                                    Task { _ = await model.performTaskAction(candidate, operation: .confirmHard) }
                                }
                                Button("Make soft") {
                                    Task { _ = await model.performTaskAction(candidate, operation: .downgradeToSoft) }
                                }
                            }
                            .buttonStyle(.bordered)
                        }
                    } else if !model.canWriteTasks {
                        VStack(alignment: .leading, spacing: 8) {
                            Label("Task actions are locked on this iPhone", systemImage: "lock")
                                .font(.footnote.weight(.semibold))
                                .foregroundStyle(BrunnTheme.amber)
                            if !model.isDemo, model.connectionValidated {
                                Button {
                                    Task { await model.bootstrapDeviceTaskAccess() }
                                } label: {
                                    Text(
                                        model.isConfiguringDeviceTaskAccess
                                            ? "Enabling…"
                                            : model.hasStoredDeviceTaskCredential
                                                ? "Repair task actions"
                                                : "Enable task actions"
                                    )
                                    .frame(maxWidth: .infinity, minHeight: 44)
                                }
                                .buttonStyle(.borderedProminent)
                                .disabled(model.isConfiguringDeviceTaskAccess)
                                .accessibilityIdentifier("task-detail-enable-actions")
                            } else {
                                Text("Connect this iPhone to enable task actions.")
                                    .font(.footnote)
                                    .foregroundStyle(.secondary)
                            }
                            if let message = model.deviceTaskAccessMessage {
                                Text(message)
                                    .font(.footnote)
                                    .foregroundStyle(BrunnTheme.amber)
                            }
                        }
                        .accessibilityIdentifier("task-detail-view-only")
                    }
                }
                .padding(16)
                .frame(maxWidth: 720)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(BrunnTheme.canvas)
            .navigationTitle("Task")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                        .accessibilityIdentifier("task-detail-close")
                }
            }
            .accessibilityIdentifier("task-detail")
        }
    }
}

private struct SourceLine: View {
    let source: String

    private var label: String {
        switch source {
        case "owner": "Set by you"
        case "todoist": "Imported from Todoist"
        default: "Inferred by \(source)"
        }
    }

    var body: some View {
        Label(
            label,
            systemImage: source == "owner" ? "person.crop.circle" : "wand.and.stars"
        )
        .font(.caption)
        .foregroundStyle(source == "owner" ? .secondary : BrunnTheme.amber)
    }
}

private struct AgentTaskProjectDetailView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss
    let state: AgentTaskProjectStateData

    private var taskProjection: AgentTaskProjectProjection {
        .bounded(next: state.next, waiting: state.waiting)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text(state.project.title)
                    .font(.largeTitle.bold())
                    .foregroundStyle(BrunnTheme.ink)
                HStack {
                    StatusPill(text: state.project.interest.uppercased(), color: BrunnTheme.pulse)
                    Text("\(state.urgentCount) urgent · \(state.parkedCount) parked")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                if let checkpoint = state.checkpoint {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Latest checkpoint")
                            .font(.headline)
                        if let objective = checkpoint.state?.objective {
                            Text(objective).font(.body)
                        }
                        ForEach(checkpoint.state?.currentState?.lines ?? [], id: \.self) {
                            Label($0, systemImage: "circle.fill")
                                .font(.subheadline)
                        }
                        ForEach(checkpoint.state?.nextActions?.lines ?? [], id: \.self) {
                            Label($0, systemImage: "arrow.right")
                                .font(.subheadline)
                        }
                    }
                    .padding(12)
                    .background(.background, in: RoundedRectangle(cornerRadius: 8))
                    .overlay {
                        RoundedRectangle(cornerRadius: 8).stroke(BrunnTheme.line, lineWidth: 1)
                    }
                }

                if !taskProjection.next.isEmpty {
                    Text("Next 3").font(.headline)
                    ForEach(taskProjection.next) { candidate in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(candidate.title).font(.subheadline.weight(.semibold))
                            Text(candidate.reason).font(.caption).foregroundStyle(.secondary)
                        }
                        .accessibilityIdentifier("task-project-next-\(candidate.taskRef)")
                    }
                }

                if !taskProjection.waiting.isEmpty {
                    Text("Waiting on").font(.headline)
                    ForEach(taskProjection.waiting) { item in
                        LabeledContent(item.title, value: "\(item.ageDays)d")
                            .accessibilityIdentifier("task-project-waiting-\(item.taskRef)")
                    }
                }

                if state.next.count + state.waiting.count > taskProjection.taskCount {
                    Text("\(state.next.count + state.waiting.count - taskProjection.taskCount) more tasks stay in the project backlog")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("task-project-remaining")
                }

                if model.canWriteTasks {
                    HStack {
                        ForEach(["hot", "normal", "parked"], id: \.self) { interest in
                            if interest == state.project.interest {
                                Button(interest.capitalized) {
                                    Task { await model.setProjectInterest(interest) }
                                }
                                .buttonStyle(.borderedProminent)
                            } else {
                                Button(interest.capitalized) {
                                    Task { await model.setProjectInterest(interest) }
                                }
                                .buttonStyle(.bordered)
                            }
                        }
                    }
                    .accessibilityIdentifier("task-project-interest")
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(BrunnTheme.canvas)
        .navigationTitle("Project")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .confirmationAction) {
                Button("Done") { dismiss() }
            }
        }
        .accessibilityIdentifier("task-project-detail")
    }
}

private struct ConnectionBanner: View {
    let message: String
    let isDemo: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: isDemo ? "sparkles" : "wifi.exclamationmark")
                .foregroundStyle(isDemo ? BrunnTheme.pulse : BrunnTheme.amber)
                .accessibilityHidden(true)
            Text(message)
                .font(.footnote)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(11)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background, in: RoundedRectangle(cornerRadius: 6))
        .overlay {
            RoundedRectangle(cornerRadius: 6)
                .stroke(BrunnTheme.line, lineWidth: 1)
        }
    }
}

struct BriefingReader: View {
    let briefing: BriefingEditionData
    let cachedAt: Date?
    let focusedItemID: String?

    @State private var showAllSummary = true
    @State private var expandedItems: Set<String> = []
    @State private var showsHistory = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            BriefingReaderHeader(briefing: briefing, cachedAt: cachedAt)

            if let payload = briefing.briefing {
                if let summary = payload.summaryMD, !summary.isEmpty {
                    BriefingSummary(
                        lines: summary,
                        showAll: $showAllSummary
                    )
                }

                ForEach(BriefingDisplaySection.grouped(payload.sections ?? [])) { section in
                    BriefingSectionView(
                        section: section,
                        expandedItems: $expandedItems,
                        toggle: toggle
                    )
                }
            } else {
                LegacyBriefing(markdown: briefing.markdown)
            }

            if !briefing.versions.isEmpty {
                BriefingRevisionHistory(
                    briefing: briefing,
                    isExpanded: $showsHistory
                )
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("briefing-reader")
        .onAppear {
            if let focusedItemID {
                expandedItems.insert(focusedItemID)
            }
        }
        .onChange(of: focusedItemID) { _, newValue in
            if let newValue {
                expandedItems.insert(newValue)
            }
        }
    }

    private func toggle(_ id: String) {
        let update = {
            if expandedItems.contains(id) {
                expandedItems.remove(id)
            } else {
                expandedItems.insert(id)
            }
        }
        if reduceMotion {
            update()
        } else {
            withAnimation(.easeInOut(duration: 0.18), update)
        }
    }
}

private struct BriefingReaderHeader: View {
    let briefing: BriefingEditionData
    let cachedAt: Date?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.title2.bold())
                .foregroundStyle(BrunnTheme.ink)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityIdentifier("briefing-reader-title")

            Text(publicationLine)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            if let cachedAt {
                Label(
                    "Protected offline copy saved \(DisplayDate.relative(cachedAt)). Pull to retry.",
                    systemImage: "lock.fill"
                )
                .font(.caption)
                .foregroundStyle(BrunnTheme.amber)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var title: String {
        "\(briefing.edition.replacingOccurrences(of: "_", with: " ").localizedCapitalized) briefing - \(briefing.date)"
    }

    private var publicationLine: String {
        let timeZone = briefing.briefing?.timezone
        let generated = BriefingReaderDate.dateTime(
            briefing.briefing?.generatedAt ?? briefing.createdAt,
            timeZoneIdentifier: timeZone
        ) ?? "Unknown"
        var text = "Generated \(generated)"
        if briefing.currentVersion > 1,
           let latest = briefing.versions.last,
           let updated = BriefingReaderDate.dateTime(
               latest.createdAt,
               timeZoneIdentifier: timeZone
           )
        {
            text += " · Updated \(updated)"
        }
        return text
    }
}

private struct BriefingSummary: View {
    let lines: [String]
    @Binding var showAll: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("30-SECOND SUMMARY")
                .font(.caption.weight(.bold))
                .foregroundStyle(.secondary)

            VStack(alignment: .leading, spacing: 10) {
                ForEach(Array(visibleLines.enumerated()), id: \.offset) { index, line in
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        Circle()
                            .fill(BrunnTheme.ink)
                            .frame(width: 5, height: 5)
                            .accessibilityHidden(true)
                        SafeMarkdownText(markdown: line)
                            .font(.body)
                            .foregroundStyle(BrunnTheme.ink)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .fixedSize(horizontal: false, vertical: true)
                            .textSelection(.enabled)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("briefing-summary-line-\(index)")
                }
            }

            if lines.count > previewCount {
                Button(showAll ? "Show less" : "\(lines.count - previewCount) more") {
                    let update = { showAll.toggle() }
                    if reduceMotion {
                        update()
                    } else {
                        withAnimation(.easeInOut(duration: 0.18), update)
                    }
                }
                .buttonStyle(.plain)
                .font(.body.weight(.semibold))
                .foregroundStyle(BrunnTheme.ink)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .frame(minHeight: 44)
                .background(.background, in: RoundedRectangle(cornerRadius: 6))
                .overlay {
                    RoundedRectangle(cornerRadius: 6)
                        .stroke(Color(uiColor: .separator), lineWidth: 1)
                }
                .accessibilityLabel(
                    showAll
                        ? "Show fewer summary items"
                        : "Show all \(lines.count) summary items"
                )
                .accessibilityIdentifier("briefing-summary-toggle")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 13)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background, in: RoundedRectangle(cornerRadius: 6))
        .overlay {
            RoundedRectangle(cornerRadius: 6)
                .stroke(BrunnTheme.line, lineWidth: 1)
        }
        .overlay(alignment: .leading) {
            UnevenRoundedRectangle(
                topLeadingRadius: 6,
                bottomLeadingRadius: 6
            )
            .fill(BrunnTheme.signal)
            .frame(width: 3)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("briefing-summary")
    }

    private let previewCount = 3

    private var visibleLines: [String] {
        showAll ? lines : Array(lines.prefix(previewCount))
    }
}

private struct BriefingSectionView: View {
    let section: BriefingDisplaySection
    @Binding var expandedItems: Set<String>
    let toggle: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: 9) {
                Text(section.title)
                    .font(.headline)
                    .foregroundStyle(BrunnTheme.ink)
                Text("\(section.itemCount) \(section.itemCount == 1 ? "item" : "items")")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)

            Divider()

            ForEach(section.parts) { part in
                ForEach(part.section.items) { item in
                    BriefingItemDisclosure(
                        item: item,
                        sectionTitle: part.itemLabel,
                        isExpanded: expandedItems.contains(item.id),
                        onToggle: { toggle(item.id) }
                    )
                    .id(item.id)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(Color(uiColor: .separator))
                .frame(height: 1)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(
            "briefing-section-\(section.parts.map(\.section.topic).joined(separator: "-"))"
        )
    }
}

private struct BriefingItemDisclosure: View {
    let item: BriefingItem
    let sectionTitle: String
    let isExpanded: Bool
    let onToggle: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button(action: onToggle) {
                VStack(alignment: .leading, spacing: 5) {
                    Text(sectionTitle.uppercased())
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)

                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        SafeMarkdownText(markdown: item.headlineMD)
                            .font(.headline)
                            .foregroundStyle(BrunnTheme.ink)
                            .multilineTextAlignment(.leading)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .fixedSize(horizontal: false, vertical: true)

                        if let delta = item.delta {
                            DeltaPill(delta: delta)
                        }

                        Image(systemName: "chevron.down")
                            .font(.caption.weight(.bold))
                            .foregroundStyle(.secondary)
                            .rotationEffect(.degrees(isExpanded ? 180 : 0))
                            .accessibilityHidden(true)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .padding(.horizontal, 12)
            .padding(.vertical, 11)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .accessibilityLabel("\(sectionTitle). \(plainHeadline). \(isExpanded ? "Expanded" : "Collapsed")")
            .accessibilityIdentifier("briefing-item-\(item.id)")

            if isExpanded {
                BriefingItemDetail(item: item)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 12)
                    .transition(.opacity.combined(with: .move(edge: .top)))
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("briefing-item-detail-\(item.id)")
            }

            Divider()
        }
    }

    private var plainHeadline: String {
        String(SafeMarkdown.attributedString(item.headlineMD).characters)
    }
}

private struct DeltaPill: View {
    let delta: String

    var body: some View {
        Text(label)
            .font(.caption2.weight(.bold))
            .foregroundStyle(tint)
            .padding(.horizontal, 7)
            .padding(.vertical, 4)
            .background(tint.opacity(0.09), in: RoundedRectangle(cornerRadius: 5))
            .overlay {
                RoundedRectangle(cornerRadius: 5)
                    .stroke(tint.opacity(0.32), lineWidth: 1)
            }
            .fixedSize()
    }

    private var label: String {
        switch delta {
        case "update": "UPDATE"
        case "corroboration": "SEEN"
        case "correction": "CORRECTION"
        default: "NEW"
        }
    }

    private var tint: Color {
        switch delta {
        case "update": BrunnTheme.pulse
        case "corroboration": BrunnTheme.amber
        case "correction": BrunnTheme.red
        default: BrunnTheme.signal
        }
    }
}

private struct BriefingItemDetail: View {
    let item: BriefingItem

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let body = item.bodyMD, !body.isEmpty {
                SafeMarkdownText(markdown: body)
                    .font(.body)
                    .foregroundStyle(BrunnTheme.ink)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }

            if let detail = item.detailMD, !detail.isEmpty {
                SafeMarkdownText(markdown: detail)
                    .font(.body)
                    .foregroundStyle(BrunnTheme.ink)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }

            if let changed = item.whatChanged, !changed.isEmpty {
                LabeledDetail(title: "What changed", text: changed)
            }

            if let why = item.whyItMatters, !why.isEmpty {
                LabeledDetail(title: "Why it matters", text: why)
            }

            ItemProvenance(item: item)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 6))
        .overlay {
            RoundedRectangle(cornerRadius: 6)
                .stroke(BrunnTheme.line, lineWidth: 1)
        }
    }
}

private struct LabeledDetail: View {
    let title: String
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title)
                .font(.subheadline.italic())
                .foregroundStyle(.secondary)
            Text(text)
                .font(.body)
                .foregroundStyle(BrunnTheme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
        }
    }
}

private struct ItemProvenance: View {
    let item: BriefingItem

    private var safeURLs: [URL] {
        (item.story?.urls ?? []).compactMap(URL.init(string:)).filter {
            $0.scheme?.lowercased() == "https" || $0.scheme?.lowercased() == "http"
        }
    }

    private var timestamps: [String] {
        let values: [(String, String?)] = [
            ("Published", item.times?.publishedAt),
            ("Event", item.times?.eventAt ?? item.story?.eventAt),
            ("First seen", item.times?.firstSeenAt),
        ]
        return values.compactMap { label, value in
            guard let value, !value.isEmpty else { return nil }
            return "\(label) \(BriefingReaderDate.compact(value) ?? value)"
        }
    }

    var body: some View {
        if !safeURLs.isEmpty || !timestamps.isEmpty {
            VStack(alignment: .leading, spacing: 7) {
                if !safeURLs.isEmpty {
                    Text("SOURCES")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(.secondary)

                    ForEach(safeURLs, id: \.absoluteString) { url in
                        Link(destination: url) {
                            Label(sourceLabel(url), systemImage: "arrow.up.right.square")
                                .font(.subheadline.weight(.semibold))
                                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                        }
                    }
                }

                if !timestamps.isEmpty {
                    Text(timestamps.joined(separator: " · "))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private func sourceLabel(_ url: URL) -> String {
        (url.host ?? "Open source").replacingOccurrences(of: "www.", with: "")
    }
}

private struct LegacyBriefing: View {
    let markdown: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Briefing")
                    .font(.headline)
                Spacer()
                StatusPill(text: "Legacy Markdown", color: BrunnTheme.amber)
            }
            Divider()
            SafeMarkdownText(markdown: markdown)
                .font(.body)
                .textSelection(.enabled)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(Color(uiColor: .separator))
                .frame(height: 1)
        }
    }
}

private struct BriefingRevisionHistory: View {
    let briefing: BriefingEditionData
    @Binding var isExpanded: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                let update = { isExpanded.toggle() }
                if reduceMotion {
                    update()
                } else {
                    withAnimation(.easeInOut(duration: 0.18), update)
                }
            } label: {
                HStack(spacing: 10) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Briefing history")
                            .font(.headline)
                            .foregroundStyle(BrunnTheme.ink)
                        Text(historySummary)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 0)
                    Image(systemName: "chevron.down")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(.secondary)
                        .rotationEffect(.degrees(isExpanded ? 180 : 0))
                        .accessibilityHidden(true)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            .accessibilityLabel("Briefing history, \(historySummary), \(isExpanded ? "expanded" : "collapsed")")
            .accessibilityIdentifier("briefing-revision-history")

            if isExpanded {
                Divider()
                VStack(spacing: 0) {
                    ForEach(briefing.versions.reversed()) { version in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text("Version \(version.version)")
                                .font(.subheadline.weight(.semibold))
                            if version.version == briefing.currentVersion {
                                Text("CURRENT")
                                    .font(.caption2.weight(.bold))
                                    .foregroundStyle(BrunnTheme.signal)
                            }
                            Spacer(minLength: 8)
                            Text(
                                BriefingReaderDate.dateTime(
                                    version.createdAt,
                                    timeZoneIdentifier: briefing.briefing?.timezone
                                ) ?? version.createdAt
                            )
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.trailing)
                        }
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .accessibilityElement(children: .combine)
                        .accessibilityIdentifier("briefing-version-\(version.version)")

                        if version.version != briefing.versions.first?.version {
                            Divider()
                                .padding(.leading, 14)
                        }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(Color(uiColor: .separator))
                .frame(height: 1)
        }
    }

    private var historySummary: String {
        let count = briefing.versions.count
        let materialChanges = (briefing.briefing?.delta?.added.count ?? 0)
            + (briefing.briefing?.delta?.changed.count ?? 0)
            + (briefing.briefing?.delta?.removed.count ?? 0)
        let versions = "\(count) \(count == 1 ? "version" : "versions")"
        guard briefing.currentVersion > 1 else { return versions }
        let changes = "\(materialChanges) material \(materialChanges == 1 ? "change" : "changes")"
        return "\(versions) · \(changes) in the latest revision"
    }
}

private enum BriefingReaderDate {
    static func dateTime(_ value: String?, timeZoneIdentifier: String? = nil) -> String? {
        guard let value, let date = parse(value) else { return nil }
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateStyle = .medium
        formatter.timeStyle = .short
        if let timeZoneIdentifier, let timeZone = TimeZone(identifier: timeZoneIdentifier) {
            formatter.timeZone = timeZone
        }
        return formatter.string(from: date)
    }

    static func compact(_ value: String) -> String? {
        if value.count == 10 {
            let formatter = DateFormatter()
            formatter.locale = Locale(identifier: "en_US_POSIX")
            formatter.dateFormat = "yyyy-MM-dd"
            guard let date = formatter.date(from: value) else { return nil }
            formatter.locale = .current
            formatter.dateStyle = .medium
            formatter.timeStyle = .none
            return formatter.string(from: date)
        }
        return dateTime(value)
    }

    private static func parse(_ value: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: value) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: value)
    }
}
