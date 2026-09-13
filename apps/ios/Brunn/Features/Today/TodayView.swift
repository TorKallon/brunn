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

                    if let status = model.dreamingStatus, status.accountLabel != nil || status.dreamer?.runtime?.usage != nil {
                        DreamingUsageCard(status: status)
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

/// The Codex account Dreaming runs under and how much of its weekly plan
/// allowance is left, as the last verified run observed it.
struct DreamingUsageCard: View {
    let status: DreamingStatusData

    private var weekly: DreamingStatusData.UsageWindow? { status.dreamer?.runtime?.usage?.weekly }

    /// Fraction of the weekly allowance still available, 0...1.
    private var remaining: Double? {
        guard let used = weekly?.usedPercent else { return nil }
        return min(max(1 - used / 100, 0), 1)
    }

    private var headline: String {
        guard let remaining else { return "Dreaming allowance not observed yet" }
        return "Dreaming has \(Int((remaining * 100).rounded()))% of its weekly allowance left"
    }

    private var detail: String {
        var parts: [String] = []
        if let account = status.accountLabel {
            parts.append(status.planLabel.map { "\(account) (\($0))" } ?? account)
        } else {
            parts.append("No Dreaming account connected")
        }
        if let resets = weekly?.resetsAt, let date = ISO8601DateFormatter().date(from: resets) {
            parts.append("resets " + date.formatted(date: .abbreviated, time: .shortened))
        }
        return parts.joined(separator: " · ")
    }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "moon.zzz")
                .font(.footnote)
                .foregroundStyle(BrunnTheme.signal)
                .padding(.top, 1)
            VStack(alignment: .leading, spacing: 5) {
                Text(headline)
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(BrunnTheme.ink)
                if let remaining {
                    ProgressView(value: remaining)
                        .tint(BrunnTheme.signal)
                        .frame(maxWidth: 220)
                }
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.leading)
            }
            Spacer(minLength: 0)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background, in: RoundedRectangle(cornerRadius: 8))
        .overlay { RoundedRectangle(cornerRadius: 8).stroke(BrunnTheme.line, lineWidth: 1) }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("today-dreaming-usage")
    }
}

private let taskDeletionMessage = "This removes this task from active lists, not its history. You can restore it on the web from All tasks → Deleted. Other recurring occurrences are unchanged."

struct AgentTasksView: View {
    @State private var showsTiming = false
    @EnvironmentObject private var model: AppModel
    @State private var moreTapCount = 0
    @State private var showsBoundedList = false
    @State private var snoozeCandidate: AgentTaskCandidate?
    @State private var snoozeDate = Date.now.addingTimeInterval(86_400)
    @State private var waitCandidate: AgentTaskCandidate?
    @State private var waitingOn = ""
    @State private var deleteCandidate: AgentTaskCandidate?

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
        model.nextTasks
            .uniqued(by: \.taskRef)
            .filter { $0.tier > 2 && Set($0.requiredContexts).isSubset(of: model.selectedTaskContexts) }
    }

    private var remainingReadyCount: Int {
        model.taskNextRemaining + max(matchingFetchedTasks.count - projection.next.count, 0)
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
                    if model.taskDeferralUndo != nil {
                        Button("Undo deferral") { Task { await model.undoTaskDeferral() } }
                            .buttonStyle(.bordered).frame(minHeight: 44)
                    }
                    if let captured = model.lastCapturedTaskRef {
                        Button("Add timing to captured task") { Task { await model.openTask(reference: captured) } }
                            .buttonStyle(.bordered).frame(minHeight: 44)
                    }

                    AgentTaskSurface(
                        projection: projection,
                        todayTasks: model.todayTasks,
                        quickTasks: model.quickTasks,
                        doneToday: model.doneToday,
                        projects: model.taskProjects,
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
                        capture: { text in await model.captureTodayTask(text) },
                        complete: complete,
                        open: { candidate in
                            Task { await model.openTask(reference: candidate.taskRef) }
                        },
                        action: perform,
                        delete: { deleteCandidate = $0 },
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
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Timing-sensitive", systemImage: "calendar.badge.clock") { showsTiming = true }
                        .accessibilityIdentifier("task-timing-open")
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
            .sheet(isPresented: $showsTiming) {
                NavigationStack {
                    List {
                        Text("Upcoming, deferred, and timing to clarify. Unknown timing does not schedule a reminder.")
                            .font(.footnote).foregroundStyle(.secondary)
                        if let message = model.taskMessage { Text(message).foregroundStyle(BrunnTheme.amber) }
                        ForEach(model.timingTasks) { candidate in
                            Button {
                                showsTiming = false
                                Task { await model.openTask(reference: candidate.taskRef) }
                            } label: {
                                VStack(alignment: .leading) {
                                    Text(candidate.title).foregroundStyle(BrunnTheme.ink)
                                    Text(candidate.hardDue != nil ? candidate.reason : candidate.timing?.reason ?? candidate.reason).font(.caption).foregroundStyle(.secondary)
                                    if let readyAt = candidate.readyAt { Text("Resurfaces: \(readyAt)").font(.caption) }
                                }
                            }
                        }
                        if model.isLoadingTaskTiming { ProgressView() }
                        if model.timingNextCursor != nil {
                            Button("More") { Task { await model.loadTimingTasks(reset: false) } }
                        }
                        Button("Refresh") { Task { await model.loadTimingTasks() } }
                    }
                    .navigationTitle("Timing-sensitive")
                    .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { showsTiming = false } } }
                    .task { await model.loadTimingTasks() }
                }
            }
            .alert(
                "Delete task?",
                isPresented: Binding(
                    get: { deleteCandidate != nil },
                    set: { if !$0 { deleteCandidate = nil } }
                ),
                presenting: deleteCandidate
            ) { candidate in
                Button("Delete task", role: .destructive) {
                    perform(candidate, .drop)
                }
                Button("Cancel", role: .cancel) {}
            } message: { candidate in
                Text("\(candidate.title)\n\n\(taskDeletionMessage)")
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
    let todayTasks: [AgentTaskCandidate]
    let quickTasks: [AgentTaskCandidate]
    let doneToday: AgentTaskDoneSummaryData?
    let projects: [AgentTaskProject]
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
    let capture: (String) async -> Bool
    let complete: (AgentTaskCandidate) -> Void
    let open: (AgentTaskCandidate) -> Void
    let action: (AgentTaskCandidate, AgentTaskUpdateOperation) -> Void
    let delete: (AgentTaskCandidate) -> Void
    let pickSnooze: (AgentTaskCandidate) -> Void
    let waitOn: (AgentTaskCandidate) -> Void
    let more: () -> Void
    let openProject: (AgentTaskProject) -> Void

    @State private var doneExpanded = false
    @State private var captureText = ""
    @State private var urgentExpanded = false
    @State private var quickExpanded = false

    private var visibleUrgent: [AgentTaskCandidate] {
        projection.attentionPreview(expanded: urgentExpanded)
    }
    private var remainingToday: [AgentTaskCandidate] {
        let placed = Set(projection.all.map(\.taskRef))
        return todayTasks.filter { !placed.contains($0.taskRef) }
    }
    private var remainingQuick: [AgentTaskCandidate] {
        let placed = Set((projection.all + todayTasks).map(\.taskRef))
        return quickTasks.filter { !placed.contains($0.taskRef) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if !projects.isEmpty {
                ProjectStatusStrip(projects: projects, open: openProject)
            }

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

            if !projection.urgent.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    TaskSectionHeader(
                        title: "NEEDS ATTENTION",
                        count: projection.urgent.count,
                        tint: BrunnTheme.red
                    )
                    ForEach(visibleUrgent) { candidate in
                        actionRow(candidate)
                    }
                    if projection.urgent.count > visibleUrgent.count || urgentExpanded {
                        Button(urgentExpanded ? "Show less" : "Show all \(projection.urgent.count)") { urgentExpanded.toggle() }
                            .frame(minHeight: 44)
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

                Text(nextRemaining == 0 ? "No more ready tasks" : "\(nextRemaining) more ready · backlog stays hidden")
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

            VStack(alignment: .leading, spacing: 8) {
                TaskSectionHeader(
                    title: "TODAY",
                    count: todayTasks.count,
                    tint: BrunnTheme.signal
                )
                TextField("Add for today", text: $captureText)
                    .textFieldStyle(.roundedBorder)
                    .submitLabel(.done)
                    .onSubmit(submitCapture)
                    .disabled(!canWrite)
                    .accessibilityIdentifier("task-today-capture")
                if remainingToday.isEmpty {
                    Text("Nothing captured for today.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                        .accessibilityIdentifier("task-today-empty")
                } else {
                    ForEach(remainingToday) { candidate in
                        actionRow(candidate, trailer: ageBadge(candidate), inToday: true)
                    }
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("task-today")

            VStack(alignment: .leading, spacing: 8) {
                TaskSectionHeader(
                    title: "QUICK",
                    count: remainingQuick.count,
                    tint: BrunnTheme.pulse
                )
                if remainingQuick.isEmpty {
                    Text("No quick tasks are ready.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                        .accessibilityIdentifier("task-quick-empty")
                } else {
                    ForEach(Array(remainingQuick.prefix(quickExpanded ? 25 : 3))) { candidate in
                        actionRow(candidate, trailer: candidate.estimateMinutes.map { "\($0) min" })
                    }
                    if remainingQuick.count > 3 {
                        Button(quickExpanded ? "Show less" : "More quick tasks") { quickExpanded.toggle() }.frame(minHeight: 44)
                    }
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("task-quick")

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
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("agent-task-surface")
    }

    private func submitCapture() {
        let text = captureText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        captureText = ""
        Task {
            let accepted = await capture(text)
            if !accepted, captureText.isEmpty {
                captureText = text
            }
        }
    }

    /// "1d", "2d", … since the task joined today's list; nothing on its first day.
    private func ageBadge(_ candidate: AgentTaskCandidate) -> String? {
        guard let days = OwnerDay.daysSince(candidate.todaySince), days > 0 else { return nil }
        return "\(days)d"
    }

    @ViewBuilder
    private func actionRow(
        _ candidate: AgentTaskCandidate,
        trailer: String? = nil,
        inToday: Bool = false
    ) -> some View {
        let row = AgentTaskCandidateRow(
            candidate: candidate,
            canWrite: canWrite,
            isMutating: mutatingRefs.contains(candidate.taskRef),
            trailer: trailer,
            tomorrow: canWrite ? { action(candidate, .tomorrow) } : nil,
            complete: { complete(candidate) },
            open: { open(candidate) }
        )
        if canWrite {
            row.contextMenu {
                if inToday {
                    Button("Complete", systemImage: "checkmark.circle") {
                        complete(candidate)
                    }
                    Button("Move to queue", systemImage: "tray.and.arrow.down") {
                        action(candidate, .sweep)
                    }
                }
                Button("Tomorrow", systemImage: "sunrise") {
                    action(candidate, .tomorrow)
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
                if !inToday {
                    Button(
                        candidate.pinned ? "Unpin from Today" : "Pin to Today",
                        systemImage: candidate.pinned ? "pin.slash" : "pin"
                    ) {
                        action(candidate, candidate.pinned ? .unpin : .pinToday)
                    }
                }
                if candidate.hasInferredHardDeadline {
                    Button("Confirm hard deadline", systemImage: "checkmark.seal") {
                        action(candidate, .confirmHard)
                    }
                    Button("Make it a soft due date", systemImage: "calendar.badge.minus") {
                        action(candidate, .downgradeToSoft)
                    }
                }
                Divider()
                Button("Delete task", systemImage: "trash", role: .destructive) {
                    delete(candidate)
                }
                .disabled(mutatingRefs.contains(candidate.taskRef))
            }
        } else {
            row
        }
    }
}

private extension AgentTaskProjectStatus {
    var tint: Color {
        switch self {
        case .red: BrunnTheme.red
        case .yellow: BrunnTheme.amber
        case .green: BrunnTheme.success
        case .grey: Color.secondary
        }
    }
}

/// One scrolling row of project chips: red, yellow, then green; grey
/// projects fold into a single "idle" chip until tapped.
private struct ProjectStatusStrip: View {
    let projects: [AgentTaskProject]
    let open: (AgentTaskProject) -> Void

    @State private var showsIdle = false

    private static let rank: [AgentTaskProjectStatus: Int] = [.red: 0, .yellow: 1, .green: 2]

    private var ranked: [AgentTaskProject] {
        projects.enumerated()
            .compactMap { offset, project in
                Self.rank[project.status ?? .grey].map { (project: project, order: ($0, offset)) }
            }
            .sorted { $0.order < $1.order }
            .map(\.project)
    }

    private var idle: [AgentTaskProject] {
        projects.filter { Self.rank[$0.status ?? .grey] == nil }
    }

    private var hasNoStatusYet: Bool {
        ranked.isEmpty && idle.allSatisfy { ($0.statusReason ?? "").isEmpty }
    }

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                if hasNoStatusYet {
                    chip(title: "No status yet", tint: Color.secondary)
                } else {
                    ForEach(ranked) { projectChip($0) }
                    if showsIdle {
                        ForEach(idle) { projectChip($0) }
                    } else if !idle.isEmpty {
                        Button {
                            showsIdle = true
                        } label: {
                            chip(title: "\(idle.count) idle", tint: Color.secondary)
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("task-projects-idle")
                    }
                }
            }
            .padding(.vertical, 2)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("task-projects")
    }

    private func projectChip(_ project: AgentTaskProject) -> some View {
        Button {
            open(project)
        } label: {
            chip(title: project.title, tint: (project.status ?? .grey).tint)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(project.title), \((project.status ?? .grey).rawValue)")
        .accessibilityIdentifier("task-project-\(project.slug)")
    }

    private func chip(title: String, tint: Color) -> some View {
        HStack(spacing: 6) {
            Circle()
                .fill(tint)
                .frame(width: 8, height: 8)
                .accessibilityHidden(true)
            Text(title)
                .font(.subheadline.weight(.medium))
                .foregroundStyle(BrunnTheme.ink)
                .lineLimit(1)
        }
        .padding(.horizontal, 11)
        .frame(minHeight: 36)
        .background(.background, in: Capsule())
        .overlay { Capsule().stroke(BrunnTheme.line, lineWidth: 1) }
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
    var trailer: String? = nil
    var tomorrow: (() -> Void)? = nil
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

            if let tomorrow {
                Button(action: tomorrow) { Image(systemName: "sunrise").frame(width: 44, height: 44) }
                    .buttonStyle(.plain).disabled(isMutating)
                    .accessibilityLabel("Tomorrow: \(candidate.title)")
                    .accessibilityHint("Defer to next local morning without moving the deadline")
            }

            Button(action: open) {
                VStack(alignment: .leading, spacing: 5) {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(candidate.title)
                            .font(.headline)
                            .foregroundStyle(BrunnTheme.ink)
                            .multilineTextAlignment(.leading)
                        if candidate.todaySince != nil { Text("Today").font(.caption).foregroundStyle(BrunnTheme.signal) }
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
                        if let trailer {
                            Spacer(minLength: 4)
                            Text(trailer)
                                .font(.caption.monospacedDigit())
                                .foregroundStyle(.secondary)
                                .fixedSize()
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

private struct TaskDateInput: View {
    let title: String
    @Binding var value: String
    var includesTime = false
    var timezone = TimeZone.current
    private var formatter: DateFormatter {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timezone
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter
    }
    private func text(_ date: Date) -> String {
        includesTime ? ISO8601DateFormatter().string(from: date) : formatter.string(from: date)
    }
    var body: some View {
        VStack(alignment: .leading) {
            Toggle(title, isOn: Binding(get: { !value.isEmpty }, set: { value = $0 ? text(.now) : "" }))
            if !value.isEmpty {
                DatePicker(title, selection: Binding(get: {
                    (includesTime ? ISO8601DateFormatter().date(from: value) : formatter.date(from: value)) ?? .now
                }, set: { value = text($0) }), displayedComponents: includesTime ? [.date, .hourAndMinute] : [.date])
                    .environment(\.timeZone, timezone)
            }
        }
    }
}

private struct NativeTaskTimingEditor: View {
    let task: AgentTaskDetail
    let pending: Bool
    let save: (String, AgentTaskCorrectionValue) -> Void
    @State private var softDue: String
    @State private var hardDue: String
    @State private var description: String
    @State private var severity: String
    @State private var riskKind: String
    @State private var startsOn: String
    @State private var endsOn: String
    @State private var tolerance: String
    @State private var mode: String
    @State private var days: String
    @State private var timezone: String
    @State private var anchor: String

    init(task: AgentTaskDetail, pending: Bool, save: @escaping (String, AgentTaskCorrectionValue) -> Void) {
        self.task = task; self.pending = pending; self.save = save
        let consequence = task.task.consequence?.value
        let recurrence = task.task.recurrence?.value
        _softDue = State(initialValue: task.task.softDue?.value ?? "")
        _hardDue = State(initialValue: task.task.hardDue?.value ?? "")
        _description = State(initialValue: consequence?.description ?? "")
        _severity = State(initialValue: consequence?.severity ?? "ordinary")
        _riskKind = State(initialValue: consequence?.timing?.kind ?? "unknown")
        _startsOn = State(initialValue: consequence?.timing?.startsOn ?? "")
        _endsOn = State(initialValue: consequence?.timing?.endsOn ?? "")
        _tolerance = State(initialValue: consequence?.timing?.days.map(String.init) ?? "")
        _mode = State(initialValue: recurrence?.kind == "native" ? recurrence?.mode ?? "none" : "none")
        _days = State(initialValue: recurrence?.everyDays.map(String.init) ?? "")
        _timezone = State(initialValue: recurrence?.timezone ?? TimeZone.current.identifier)
        _anchor = State(initialValue: recurrence?.anchorOn ?? "")
    }

    var body: some View {
        DisclosureGroup("Timing and recurrence") {
            VStack(alignment: .leading, spacing: 12) {
                Text("Save each field separately. An intended date is not a hard deadline. Unknown timing does not schedule a reminder.")
                    .font(.footnote).foregroundStyle(.secondary)
                TaskDateInput(title: "Intended date", value: $softDue, timezone: TimeZone(identifier: timezone) ?? .current)
                Button("Save intended date") { save("soft_due", softDue.isEmpty ? .null : .string(softDue)) }
                TaskDateInput(title: "Actual cutoff", value: $hardDue, includesTime: true)
                Button("Save cutoff") { save("hard_due", hardDue.isEmpty ? .null : .string(hardDue)) }
                Divider()
                TextField("What happens if it slips?", text: $description, axis: .vertical)
                Picker("Consequence", selection: $severity) {
                    Text("Ordinary").tag("ordinary")
                    Text("Serious loss or harm").tag("serious")
                }
                Picker("Risk timing", selection: $riskKind) {
                    Text("Not known yet").tag("unknown")
                    Text("Known date/window").tag("window")
                    Text("Days after intended date").tag("after_due")
                }
                if riskKind == "window" {
                    TaskDateInput(title: "Window starts", value: $startsOn, timezone: TimeZone(identifier: timezone) ?? .current)
                    TaskDateInput(title: "Window ends (optional)", value: $endsOn, timezone: TimeZone(identifier: timezone) ?? .current)
                } else if riskKind == "after_due" {
                    TextField("Supported tolerance in days", text: $tolerance).keyboardType(.numberPad)
                }
                Button("Save consequence") {
                    let timing: AgentTaskConsequence.RiskTiming? = riskKind == "unknown" ? nil : .init(
                        kind: riskKind, startsOn: riskKind == "window" ? startsOn : nil,
                        endsOn: riskKind == "window" && !endsOn.isEmpty ? endsOn : nil,
                        days: riskKind == "after_due" ? Int(tolerance) : nil)
                    save("consequence", description.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? .null : .consequence(.init(description: description, severity: severity, timing: timing)))
                }
                .disabled((riskKind == "after_due" && Int(tolerance).map { !(0...3650).contains($0) } != false) || (riskKind == "window" && startsOn.isEmpty))
                Divider()
                if task.task.recurrence != nil && task.task.recurrence?.value.kind != "native" {
                    Text("Imported recurrence is retained. Ask the agent to review it before replacing it.").font(.footnote)
                } else {
                    Picker("Repeat", selection: $mode) {
                        Text("Does not repeat").tag("none")
                        Text("After actual completion").tag("after_completion")
                        Text("Fixed calendar").tag("calendar")
                    }
                    if mode != "none" {
                        TextField("Every (days)", text: $days).keyboardType(.numberPad)
                        TextField("Timezone", text: $timezone)
                        if mode == "calendar" { TaskDateInput(title: "Calendar anchor", value: $anchor, timezone: TimeZone(identifier: timezone) ?? .current) }
                    }
                    Text("The intended date is the current occurrence's due date. Completing creates the next occurrence; Tomorrow does not.").font(.footnote).foregroundStyle(.secondary)
                    Button("Save recurrence") {
                        save("recurrence", mode == "none" ? .null : .recurrence(.init(kind: "native", mode: mode, everyDays: Int(days), timezone: timezone, anchorOn: mode == "calendar" ? anchor : nil)))
                    }
                    .disabled((mode != "none" && Int(days).map { !(1...3650).contains($0) } != false) || (mode == "calendar" && anchor.isEmpty))
                }
            }
            .textFieldStyle(.roundedBorder)
            .buttonStyle(.bordered)
            .disabled(pending)
        }
    }
}

private struct AgentTaskDetailView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss
    private let initialTask: AgentTaskDetail
    @State private var correctedTitle: String
    @State private var showsDeleteConfirmation = false

    private var task: AgentTaskDetail {
        model.presentedTask?.taskRef == initialTask.taskRef
            ? model.presentedTask ?? initialTask
            : initialTask
    }

    init(task: AgentTaskDetail) {
        initialTask = task
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
                            text: task.status == .dropped ? "DELETED" : task.status.rawValue.uppercased(),
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
                    if let softDue = task.task.softDue {
                        LabeledContent("Intended date", value: softDue.value)
                        SourceLine(source: softDue.source)
                    }
                    if let consequence = task.task.consequence {
                        LabeledContent(consequence.value.severity == "serious" ? "Serious consequence" : "Consequence", value: consequence.value.description)
                        SourceLine(source: consequence.source)
                    }
                    if let recurrence = task.task.recurrence {
                        Text(recurrence.value.kind == "native" ? "Every \(recurrence.value.everyDays ?? 0) days · \(recurrence.value.mode == "after_completion" ? "after actual completion" : "fixed calendar")" : "Imported recurrence (preserved)")
                        SourceLine(source: recurrence.source)
                    }
                    if model.canWriteTasks, task.status == .open || task.status == .waiting {
                        NativeTaskTimingEditor(task: task, pending: model.mutatingTaskRefs.contains(task.taskRef)) { field, value in
                            Task { _ = await model.performTaskAction(candidate, operation: .correct(field: field, value: value, note: "Owner timing on iOS")) }
                        }.id("\(task.taskRef):\(task.version)")
                        Button("Tomorrow", systemImage: "sunrise") { Task { _ = await model.performTaskAction(candidate, operation: .tomorrow) } }
                            .buttonStyle(.bordered).frame(minHeight: 44)
                    }
                    if model.taskDeferralUndo != nil {
                        Button("Undo deferral") { Task { await model.undoTaskDeferral() } }.frame(minHeight: 44)
                    }
                    if let contexts = task.task.requiredContexts, !contexts.value.isEmpty {
                        LabeledContent("Contexts", value: contexts.value.joined(separator: ", "))
                        SourceLine(source: contexts.source)
                    }

                    if let message = model.taskMessage {
                        Label(message, systemImage: "exclamationmark.triangle")
                            .font(.footnote)
                            .foregroundStyle(BrunnTheme.amber)
                            .accessibilityIdentifier("task-detail-message")
                    }

                    if model.canWriteTasks, task.status != .dropped {
                        Button("Delete task", systemImage: "trash", role: .destructive) {
                            showsDeleteConfirmation = true
                        }
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .buttonStyle(.bordered)
                        .tint(BrunnTheme.red)
                        .disabled(model.mutatingTaskRefs.contains(task.taskRef))
                        .accessibilityIdentifier("task-detail-delete")
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
            .alert("Delete task?", isPresented: $showsDeleteConfirmation) {
                Button("Delete task", role: .destructive) {
                    Task {
                        if await model.performTaskAction(candidate, operation: .drop) {
                            dismiss()
                        }
                    }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("\(task.title)\n\n\(taskDeletionMessage)")
            }
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

    private var statusLine: String {
        let reason = (state.project.statusReason ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        var text = reason.isEmpty ? "No status yet" : reason
        if let computedOn = state.project.statusComputedOn,
           let days = OwnerDay.daysSince(computedOn), days > 1
        {
            text += " · as of \(DisplayDate.metadata(computedOn))"
        }
        return text
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text(state.project.title)
                    .font(.largeTitle.bold())
                    .foregroundStyle(BrunnTheme.ink)

                HStack(alignment: .firstTextBaseline, spacing: 9) {
                    Circle()
                        .fill((state.project.status ?? .grey).tint)
                        .frame(width: 10, height: 10)
                        .accessibilityLabel((state.project.status ?? .grey).rawValue)
                    Text(statusLine)
                        .font(.body)
                        .foregroundStyle(BrunnTheme.ink)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("task-project-status")

                VStack(alignment: .leading, spacing: 8) {
                    Text("Open tasks").font(.headline)
                    HStack {
                        StatusPill(text: state.project.interest.uppercased(), color: BrunnTheme.pulse)
                        Text("\(state.urgentCount) urgent · \(state.parkedCount) parked")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    ForEach(taskProjection.next) { candidate in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(candidate.title).font(.subheadline.weight(.semibold))
                            Text(candidate.reason).font(.caption).foregroundStyle(.secondary)
                        }
                        .accessibilityIdentifier("task-project-next-\(candidate.taskRef)")
                    }
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

    @State private var expandedItemID: String?
    @State private var showsHistory = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            BriefingReaderHeader(briefing: briefing, cachedAt: cachedAt)

            if let payload = briefing.briefing {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(payload.sections ?? []) { section in
                        if !section.items.isEmpty {
                            Text(section.title.uppercased())
                                .font(.caption2.weight(.bold))
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                                .padding(.horizontal, 12)
                                .padding(.top, 14)
                                .padding(.bottom, 4)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .accessibilityAddTraits(.isHeader)
                        }
                        ForEach(section.items) { item in
                            BriefingItemDisclosure(
                                item: item,
                                sectionTitle: section.title,
                                isExpanded: expandedItemID == item.id,
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
                .accessibilityIdentifier("briefing-items")
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
                expandedItemID = focusedItemID
            }
        }
        .onChange(of: focusedItemID) { _, newValue in
            if let newValue {
                expandedItemID = newValue
            }
        }
    }

    /// One item open at a time: tapping another row swaps, tapping the open row closes.
    private func toggle(_ id: String) {
        let update = { expandedItemID = expandedItemID == id ? nil : id }
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

private struct BriefingItemDisclosure: View {
    let item: BriefingItem
    let sectionTitle: String
    let isExpanded: Bool
    let onToggle: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button(action: onToggle) {
                VStack(alignment: .leading, spacing: 5) {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(plainHeadline)
                            .font(.body)
                            .foregroundStyle(BrunnTheme.ink)
                            .multilineTextAlignment(.leading)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .fixedSize(horizontal: false, vertical: true)

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
            .padding(.vertical, 9)
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
