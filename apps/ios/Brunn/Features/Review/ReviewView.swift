import SwiftUI

struct ReviewView: View {
    @EnvironmentObject private var model: AppModel
    @StateObject private var store = DreamerReviewStore()
    @State private var filter = "all"
    @State private var showingDetail = false

    var body: some View {
        List {
            if model.isDemo && !usesReviewTestFixture {
                Section {
                    Text("Sign in to read your Dreamer proposals, evidence, and decisions.")
                } header: { Text("Dreamer Review") }
            } else {
                if let error = store.loadError {
                    Section {
                        Label("Review could not refresh", systemImage: "exclamationmark.icloud")
                        Text(error).font(.footnote)
                        Button("Try again") { Task { await refresh() } }
                    }
                }
                if let data = store.data {
                    overview(data)
                    Section {
                        Picker("Review items", selection: $filter) {
                            Text("All").tag("all")
                            Text("Proposals").tag("proposal")
                            Text("Questions").tag("question")
                        }
                        .pickerStyle(.segmented)
                        .disabled(store.selectionLocked)
                        ForEach(data.reviewItems.enumerated().sorted { left, right in
                            let leftReady = left.element.approvalBlock == nil
                            let rightReady = right.element.approvalBlock == nil
                            return leftReady == rightReady ? left.offset < right.offset : leftReady
                        }.map(\.element).filter { filter == "all" || $0.kind == filter }) { item in
                            Button {
                                store.select(item)
                                showingDetail = true
                            } label: {
                                HStack(alignment: .top, spacing: 12) {
                                    Image(systemName: item.kind == "question" ? "questionmark.bubble" : "doc.text.magnifyingglass")
                                        .foregroundStyle(BrunnTheme.signal)
                                    VStack(alignment: .leading, spacing: 6) {
                                        Text(item.title).foregroundStyle(BrunnTheme.ink)
                                            .multilineTextAlignment(.leading)
                                        Text(reviewLabel(item.status)).font(.caption)
                                            .foregroundStyle(.secondary)
                                    }
                                    Spacer(minLength: 0)
                                    Image(systemName: "chevron.right").foregroundStyle(.secondary)
                                }
                                .padding(.vertical, 7)
                            }
                            .disabled(store.selectionLocked)
                            .accessibilityIdentifier("review-item-\(item.id)")
                        }
                        if data.available && data.reviewItems.isEmpty {
                            Text("Nothing needs a decision. Older run notes are available below.")
                                .foregroundStyle(.secondary)
                        }
                    } header: { Text("\(data.reviewItems.count) for review") }
                    if store.selectionLocked && !showingDetail {
                        Button("Return to unconfirmed decision") { showingDetail = true }
                    }
                    if !data.olderReports.isEmpty {
                        Section {
                            DisclosureGroup {
                                Text("Notes from earlier runs are kept here for reference. They are not awaiting a decision.")
                                    .font(.footnote).foregroundStyle(.secondary)
                                ForEach(data.olderReports) { item in
                                    Button {
                                        store.select(item, historical: true)
                                        showingDetail = true
                                    } label: {
                                        VStack(alignment: .leading, spacing: 6) {
                                            Text(item.historicalTitle).foregroundStyle(BrunnTheme.ink)
                                                .multilineTextAlignment(.leading)
                                            Text("\(item.runID) · Older report")
                                                .font(.caption).foregroundStyle(.secondary)
                                        }
                                        .padding(.vertical, 7)
                                    }
                                    .disabled(store.selectionLocked)
                                    .accessibilityIdentifier("review-older-\(item.id)")
                                }
                            } label: {
                                Text("Older reports (\(data.olderReports.count))")
                                    .accessibilityIdentifier("review-older-reports")
                            }
                        }
                    }
                    if !data.history.isEmpty {
                        Section("Recent decisions") {
                            ForEach(data.history) { decision in
                                ReviewHistoryRow(decision: decision)
                            }
                        }
                    }
                } else if store.isRefreshing {
                    ProgressView("Loading Review…")
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(BrunnTheme.canvas)
        .navigationTitle("Review")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { Task { await refresh() } } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .accessibilityLabel("Refresh Review")
                .disabled(store.isRefreshing || model.isDemo)
            }
        }
        .navigationDestination(isPresented: $showingDetail) {
            ReviewDetailView(store: store)
        }
        .refreshable { await refresh() }
        .task { await refresh() }
        .accessibilityIdentifier("dreamer-review")
    }

    private var usesReviewTestFixture: Bool {
#if DEBUG
        model.isDemo && ProcessInfo.processInfo.arguments.contains("--ui-test-review-fixture")
#else
        false
#endif
    }

    private func refresh() async {
#if DEBUG
        if usesReviewTestFixture { store.loadUITestFixture(); return }
#endif
        guard !model.isDemo else { return }
        await store.refresh(api: model.api, userID: model.user?.id)
    }

    @ViewBuilder
    private func overview(_ data: DreamerReviewData) -> some View {
        Section {
            Text("Concrete proposals and questions that need your decision.")
            if !data.available {
                Label(data.unavailableReason ?? "A complete review snapshot is unavailable.", systemImage: "exclamationmark.triangle")
                    .foregroundStyle(BrunnTheme.amber)
            }
            LabeledContent("Application mode", value: reviewLabel(data.mode ?? "Unavailable"))
            if data.paused { Label("Dreaming paused", systemImage: "pause.circle") }
            Text(data.mode == "report-only"
                 ? "Approvals are held. No candidate is applied in report-only mode."
                 : "Approval is recorded first. Application requires source and policy checks.")
                .font(.footnote).foregroundStyle(.secondary)
            DisclosureGroup("Run status") {
                if let attempt = data.lastAttempt {
                    LabeledContent("Last attempt", value: reviewLabel(attempt.outcome))
                    Text(attempt.finishedAt ?? attempt.startedAt ?? attempt.date ?? "Time unavailable")
                        .font(.caption).foregroundStyle(.secondary)
                    if let detail = attempt.detail { Text(detail).font(.footnote) }
                } else { Text("No attempt recorded") }
                if let run = data.lastSuccessfulRun {
                    ReviewSourceLink(reference: run.entryRef, version: run.version,
                                     title: "Last successful run: \(run.runID)")
                } else { Text("No successful run recorded") }
                LabeledContent("Approved and held", value: "\(data.counts.approvedHeld)")
                LabeledContent("Applied", value: "\(data.counts.applied)")
            }
        }
    }
}

private struct ReviewDetailView: View {
    @EnvironmentObject private var model: AppModel
    @ObservedObject var store: DreamerReviewStore
    @State private var evidenceExpanded = false
    @State private var exactChangesExpanded = false

    var body: some View {
        ScrollView {
            if let item = store.displayedItem {
                VStack(alignment: .leading, spacing: 24) {
                    VStack(alignment: .leading, spacing: 10) {
                        Text(store.selectionIsHistorical ? "Older report" : item.kind == "question" ? "Needs your call" : "Proposed")
                            .font(.caption).foregroundStyle(BrunnTheme.signal)
                        Text(store.selectionIsHistorical ? item.historicalTitle : item.title).font(.title2).fontDesign(.serif)
                        if !store.selectionIsHistorical {
                            Text(reviewLabel(item.status)).font(.caption).foregroundStyle(.secondary)
                        }
                        ReviewSourceLink(reference: item.runEntryRef, version: item.runVersion,
                                         title: "\(item.runID) · report")
                    }
                    if store.selectionIsHistorical {
                        Text("Kept for reference. This note is not awaiting a decision and will not be applied.")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                    if !store.selectionIsHistorical && store.data?.mode == "report-only" {
                        Label("Report-only: approvals are held; nothing is applied.", systemImage: "pause.circle")
                            .font(.footnote).foregroundStyle(BrunnTheme.signal)
                    }
                    if !store.selectionIsHistorical && (store.changed || store.conflict) && store.decisionMessage == nil {
                        VStack(alignment: .leading, spacing: 10) {
                            Label("This review has changed", systemImage: "exclamationmark.triangle")
                            Text("Read the updated item and decisions before another action. Your note is kept.")
                            if store.data?.items.contains(where: { $0.id == item.id }) == true {
                                Button("Review updated item") { store.selectUpdated() }
                                    .disabled(store.selectionLocked)
                            } else { Text("This item is no longer pending.") }
                        }
                        .foregroundStyle(BrunnTheme.amber)
                    }
                    if !store.selectionIsHistorical, (item.kind != "question" || item.stale), let blocked = item.approvalBlock {
                        Label(blocked, systemImage: "info.circle")
                            .font(.footnote).foregroundStyle(BrunnTheme.amber)
                    }
                    ReviewTextSection(title: store.selectionIsHistorical ? "Original note" : item.kind == "question" ? "The question" : "Proposal", text: item.bodyMD)
                    if !store.selectionIsHistorical, let candidate = item.candidate, candidate.hasContent {
                        VStack(alignment: .leading, spacing: 12) {
                            Text(candidate.isManagedSummary ? "Proposed summary" : "Candidate").font(.headline)
                            if candidate.isManagedSummary, let summary = candidate.summaryMD {
                                ReviewSummaryMarkdown(text: summary)
                                DisclosureGroup(isExpanded: $exactChangesExpanded) {
                                    VStack(alignment: .leading, spacing: 12) {
                                        if let path = candidate.targetPath {
                                            Text(path).font(.caption.monospaced()).textSelection(.enabled)
                                        }
                                        if let before = candidate.beforeMD {
                                            ReviewTextSection(title: "Before", text: before.isEmpty ? "(New entry)" : before, raw: true)
                                        }
                                        ReviewTextSection(title: "After", text: candidate.afterMD ?? summary, raw: true)
                                    }.padding(.top, 12)
                                } label: {
                                    Text("View exact changes").accessibilityIdentifier("review-exact-changes-toggle")
                                }
                            } else if let before = candidate.beforeMD, let after = candidate.afterMD {
                                if let path = candidate.targetPath {
                                    Text(path).font(.caption.monospaced()).textSelection(.enabled)
                                }
                                ReviewTextSection(title: "Before", text: before.isEmpty ? "(New entry)" : before, raw: true)
                                ReviewTextSection(title: "After", text: after.isEmpty ? "(Empty content)" : after, raw: true)
                            } else if let body = candidate.bodyMD ?? candidate.afterMD {
                                if let path = candidate.targetPath {
                                    Text(path).font(.caption.monospaced()).textSelection(.enabled)
                                }
                                ReviewTextSection(title: "Proposed content", text: body)
                            }
                        }
                    }
                    if !store.selectionIsHistorical {
                        if let why = item.whyMD, !why.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                            ReviewTextSection(title: "Why it is proposed", text: why)
                        }
                        if let uncertainty = item.uncertaintyMD,
                           !uncertainty.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                           !alreadyShown(uncertainty, in: item) {
                            ReviewTextSection(title: "Uncertainty", text: uncertainty)
                        }
                    }
                    if !item.sources.isEmpty {
                        DisclosureGroup(isExpanded: $evidenceExpanded) {
                            VStack(alignment: .leading, spacing: 16) {
                                ForEach(Array(item.sources.enumerated()), id: \.offset) { _, source in
                                    VStack(alignment: .leading, spacing: 8) {
                                        ReviewSourceLink(reference: source.entryRef, version: source.version,
                                                         title: source.label ?? source.path ?? source.entryRef)
                                        if let excerpt = source.excerpt {
                                            Text(excerpt).font(.body).textSelection(.enabled)
                                        }
                                    }
                                    Divider()
                                }
                            }.padding(.top, 12)
                        } label: {
                            Text("Supporting evidence (\(item.sources.count))").font(.headline)
                                .accessibilityIdentifier("review-evidence-toggle")
                        }
                    }
                    if !store.selectionIsHistorical { decisionForm(item) }
                    if store.isSubmitting || store.decisionMessage != nil || store.decisionError != nil {
                        decisionStatus
                    }
                    let history = store.data?.history.filter { $0.itemID == item.id } ?? []
                    if !history.isEmpty {
                        VStack(alignment: .leading, spacing: 16) {
                            Text("Decisions for this item").font(.headline)
                            ForEach(history) { ReviewHistoryRow(decision: $0) }
                        }
                    }
                }
                .frame(maxWidth: 760, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(16)
            }
        }
        .background(BrunnTheme.canvas)
        .navigationTitle(store.selectionIsHistorical ? "Older report" : "Review item")
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await store.refresh(api: model.api, userID: model.user?.id) }
        .accessibilityIdentifier("review-detail")
    }

    private func alreadyShown(_ text: String, in item: DreamerReviewItem) -> Bool {
        var visible = [item.bodyMD]
        if let candidate = item.candidate {
            if candidate.isManagedSummary {
                if let summary = candidate.summaryMD { visible.append(summary) }
            } else if let before = candidate.beforeMD, let after = candidate.afterMD {
                visible += [before, after]
            } else if let body = candidate.bodyMD ?? candidate.afterMD {
                visible.append(body)
            }
        }
        return visible.contains { $0.contains(text) }
    }

    @ViewBuilder
    private func decisionForm(_ item: DreamerReviewItem) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(item.kind == "question" ? "Your answer" : "Your decision").font(.headline)
            if !store.canDecide { Text("A connected owner session is required to record decisions.").font(.footnote) }
            if item.kind != "question" && store.data?.mode == "report-only" {
                Text("Approve records your decision and holds application until an authorized mode permits it.")
                    .font(.footnote).foregroundStyle(.secondary)
            }
            if let error = store.loadError {
                Label("Decisions are unavailable until Review refreshes: \(error)", systemImage: "exclamationmark.icloud")
                    .font(.footnote).foregroundStyle(BrunnTheme.amber)
            }
            TextField("Comment (optional)", text: draft(item.id, keyPath: \.comment), axis: .vertical)
                .lineLimit(3...8).textFieldStyle(.roundedBorder)
                .disabled(store.decisionsDisabled)
                .accessibilityIdentifier("review-comment")
            if item.kind == "question" {
                correctionEditor(item, question: true)
            }
            VStack(spacing: 10) {
                if item.kind != "question" {
                    decisionButton("Approve", action: .approve, blocked: item.approvalBlock != nil)
                        .buttonStyle(.borderedProminent)
                }
                HStack(spacing: 12) {
                    decisionButton("Reject", action: .reject)
                    decisionButton("Defer", action: .defer)
                }.buttonStyle(.bordered)
            }
            if item.kind != "question" {
                DisclosureGroup("Suggest a correction") {
                    correctionEditor(item, question: false).padding(.top, 10)
                }
            }
        }
    }

    private var decisionStatus: some View {
        VStack(alignment: .leading, spacing: 14) {
            if store.isSubmitting { ProgressView("Recording your decision…") }
            if let message = store.decisionMessage {
                Label("Decision recorded", systemImage: "checkmark.circle").foregroundStyle(BrunnTheme.success)
                Text(message).font(.footnote)
            }
            if let error = store.decisionError {
                Label("Unable to confirm your decision", systemImage: "exclamationmark.triangle")
                    .foregroundStyle(BrunnTheme.amber)
                Text(error).font(.footnote)
                if store.pendingRequest != nil {
                    Text("Retry sends the same decision safely. Other actions are held until the result is confirmed.")
                        .font(.footnote)
                    Button("Retry same decision") {
                        Task { await store.retry(api: model.api, userID: model.user?.id) }
                    }.disabled(store.isSubmitting)
                }
            }
        }
    }

    private func correctionEditor(_ item: DreamerReviewItem, question: Bool) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            TextField(question ? "Your answer" : "Correction", text: draft(item.id, keyPath: \.correction), axis: .vertical)
                .lineLimit(4...10).textFieldStyle(.roundedBorder)
                .disabled(store.decisionsDisabled)
                .accessibilityIdentifier("review-correction")
            Text(question ? "Your answer is recorded for a fresh candidate and review." : "Your note is recorded for a fresh candidate and review.")
                .font(.footnote)
            decisionButton(question ? "Save answer" : "Save correction", action: .correct,
                           blocked: (store.drafts[item.id]?.correction ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .buttonStyle(.bordered)
        }
    }

    private func decisionButton(_ label: String, action: DreamerDecisionAction, blocked: Bool = false) -> some View {
        Button { Task { await store.decide(action, api: model.api, userID: model.user?.id) } } label: {
            Text(label).frame(maxWidth: .infinity, minHeight: 44)
        }
            .disabled(store.decisionsDisabled || blocked)
    }

    private func draft(_ id: String, keyPath: WritableKeyPath<DreamerReviewStore.Draft, String>) -> Binding<String> {
        Binding(
            get: { (store.drafts[id] ?? .init())[keyPath: keyPath] },
            set: { store.drafts[id, default: .init()][keyPath: keyPath] = String($0.prefix(4000)) }
        )
    }
}

// The managed location contract uses this two-column table. Render it as rows
// that wrap on a phone; keep unrecognized Markdown intact and exact bytes in
// the separate change disclosure.
enum ReviewSummaryBlock: Equatable {
    struct Stop: Equatable {
        let when: String
        let whereMD: String
    }
    case paragraph(String)
    case heading(String)
    case stops([Stop])

    static func parse(_ text: String) -> [Self] {
        text.replacingOccurrences(of: "\r\n", with: "\n")
            .components(separatedBy: "\n\n").filter { !$0.isEmpty }.map { paragraph in
                let lines = paragraph.components(separatedBy: "\n")
                if lines.count >= 3, cells(lines[0]) == ["When", "Where"],
                   let separator = cells(lines[1]), separator.allSatisfy({
                       let dashes = $0.trimmingCharacters(in: CharacterSet(charactersIn: ":"))
                       return dashes.count >= 3 && dashes.allSatisfy { $0 == "-" }
                   }) {
                    let rows = lines.dropFirst(2).compactMap(cells)
                    if rows.count == lines.count - 2 {
                        return .stops(rows.map { Stop(when: $0[0], whereMD: $0[1]) })
                    }
                }
                if lines.count == 1, let space = paragraph.firstIndex(of: " ") {
                    let prefix = paragraph[..<space]
                    if (1...6).contains(prefix.count), prefix.allSatisfy({ $0 == "#" }) {
                        return .heading(String(paragraph[paragraph.index(after: space)...]))
                    }
                }
                return .paragraph(paragraph)
            }
    }

    private static func cells(_ line: String) -> [String]? {
        var trimmed = line.trimmingCharacters(in: .whitespaces)
        if trimmed.hasPrefix("|") { trimmed.removeFirst() }
        if trimmed.hasSuffix("|") { trimmed.removeLast() }
        let values = trimmed.components(separatedBy: "|").map { $0.trimmingCharacters(in: .whitespaces) }
        return values.count == 2 && values.allSatisfy { !$0.isEmpty } ? values : nil
    }
}

private struct ReviewSummaryMarkdown: View {
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            ForEach(Array(ReviewSummaryBlock.parse(text).enumerated()), id: \.offset) { _, block in
                switch block {
                case .heading(let heading):
                    SafeMarkdownText(markdown: heading).font(.headline)
                case .paragraph(let paragraph):
                    SafeMarkdownText(markdown: paragraph).font(.body)
                case .stops(let stops):
                    VStack(alignment: .leading, spacing: 14) {
                        ForEach(Array(stops.enumerated()), id: \.offset) { _, stop in
                            VStack(alignment: .leading, spacing: 4) {
                                Text(stop.when).font(.subheadline.weight(.semibold)).foregroundStyle(.secondary)
                                SafeMarkdownText(markdown: stop.whereMD).font(.body)
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                }
            }
        }
        .textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct ReviewTextSection: View {
    let title: String
    let text: String
    var raw = false
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title).font(.headline)
            if raw { Text(text).font(.body.monospaced()).textSelection(.enabled) }
            else {
                ForEach(Array(text.components(separatedBy: "\n\n").enumerated()), id: \.offset) { _, paragraph in
                    SafeMarkdownText(markdown: paragraph).font(.body).textSelection(.enabled)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .fixedSize(horizontal: false, vertical: true)
    }
}

private struct ReviewSourceLink: View {
    let reference: String
    let version: Int?
    let title: String
    var body: some View {
        if reference.hasPrefix("entry:"), let version, version > 0 {
            NavigationLink {
                ContextSourceView(request: .init(reference: reference, version: version, title: title))
            } label: {
                Text("\(title) · v\(version)")
                    .multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
            }
        } else {
            Text(title).font(.subheadline).textSelection(.enabled)
        }
    }
}

private struct ReviewHistoryRow: View {
    let decision: DreamerReviewDecision
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("\(reviewLabel(decision.decision)) · \(reviewLabel(decision.applicationStatus))")
                .font(.subheadline)
            Text(decision.itemID).font(.caption.monospaced()).foregroundStyle(.secondary)
            Text(decision.at).font(.caption).foregroundStyle(.secondary)
            if let comment = decision.comment { Text(comment).font(.footnote).textSelection(.enabled) }
            if let correction = decision.correction { Text(correction).font(.footnote).textSelection(.enabled) }
        }
    }
}

private func reviewLabel(_ value: String) -> String {
    value.replacingOccurrences(of: "_", with: " ").replacingOccurrences(of: "-", with: " ").capitalized
}
