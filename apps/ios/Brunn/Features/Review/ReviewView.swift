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
                        ForEach(data.items.enumerated().sorted { left, right in
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
                        if data.available && data.items.isEmpty {
                            Text("No pending items. Your decisions remain below.")
                                .foregroundStyle(.secondary)
                        }
                    } header: { Text("\(data.counts.pending) pending") }
                    if store.selectionLocked && !showingDetail {
                        Button("Return to unconfirmed decision") { showingDetail = true }
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
            Text("Proposals and questions from nightly dreaming.")
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

    var body: some View {
        ScrollView {
            if let item = store.displayedItem {
                VStack(alignment: .leading, spacing: 24) {
                    VStack(alignment: .leading, spacing: 10) {
                        Text(item.kind == "question" ? "Needs your call" : "Proposed")
                            .font(.caption).foregroundStyle(BrunnTheme.signal)
                        Text(item.title).font(.title2).fontDesign(.serif)
                        Text(reviewLabel(item.status)).font(.caption).foregroundStyle(.secondary)
                        ReviewSourceLink(reference: item.runEntryRef, version: item.runVersion,
                                         title: "\(item.runID) · report v\(item.runVersion)")
                    }
                    if store.data?.mode == "report-only" {
                        Label("Report-only: approvals are held; nothing is applied.", systemImage: "pause.circle")
                            .font(.footnote).foregroundStyle(BrunnTheme.signal)
                    }
                    if (store.changed || store.conflict) && store.decisionMessage == nil {
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
                    if let blocked = item.approvalBlock {
                        Label(blocked, systemImage: "info.circle")
                            .font(.footnote).foregroundStyle(BrunnTheme.amber)
                    }
                    ReviewTextSection(title: item.kind == "question" ? "The question" : "Proposal", text: item.bodyMD)
                    if let candidate = item.candidate {
                        VStack(alignment: .leading, spacing: 12) {
                            Text("Candidate").font(.headline)
                            if let path = candidate.targetPath {
                                Text(path).font(.caption.monospaced()).textSelection(.enabled)
                            }
                            if let before = candidate.beforeMD, let after = candidate.afterMD {
                                ReviewTextSection(title: "Before", text: before.isEmpty ? "(New entry)" : before, raw: true)
                                ReviewTextSection(title: "After", text: after.isEmpty ? "(Empty content)" : after, raw: true)
                            } else if let body = candidate.bodyMD ?? candidate.afterMD {
                                ReviewTextSection(title: "Proposed content", text: body)
                            }
                        }
                    }
                    ReviewTextSection(title: "Why it is proposed", text: item.whyMD ?? "No separate rationale was supplied.")
                    ReviewTextSection(title: "Uncertainty", text: item.uncertaintyMD ?? "No uncertainty statement was supplied.")
                    VStack(alignment: .leading, spacing: 16) {
                        Text("Supporting evidence · \(item.sources.count)").font(.headline)
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
                        if item.sources.isEmpty { Text("No exact source references are attached yet.").foregroundStyle(.secondary) }
                    }
                    decisionForm(item)
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
        .navigationTitle("Review item")
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await store.refresh(api: model.api, userID: model.user?.id) }
        .accessibilityIdentifier("review-detail")
    }

    @ViewBuilder
    private func decisionForm(_ item: DreamerReviewItem) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Your decision").font(.headline)
            if !store.canDecide { Text("A connected owner session is required to record decisions.").font(.footnote) }
            if store.data?.mode == "report-only" {
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
            VStack(spacing: 10) {
                decisionButton("Approve", action: .approve, blocked: item.approvalBlock != nil)
                    .buttonStyle(.borderedProminent)
                HStack(spacing: 12) {
                    decisionButton("Reject", action: .reject)
                    decisionButton("Defer", action: .defer)
                }.buttonStyle(.bordered)
            }
            DisclosureGroup(item.kind == "question" ? "Answer or correct this item" : "Suggest a correction") {
                VStack(alignment: .leading, spacing: 12) {
                    TextField("Answer or correction", text: draft(item.id, keyPath: \.correction), axis: .vertical)
                        .lineLimit(4...10).textFieldStyle(.roundedBorder)
                        .disabled(store.decisionsDisabled)
                        .accessibilityIdentifier("review-correction")
                    Text("Your note is recorded for a fresh candidate and review.").font(.footnote)
                    decisionButton("Save correction", action: .correct,
                                   blocked: (store.drafts[item.id]?.correction ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .buttonStyle(.bordered)
                }.padding(.top, 10)
            }
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
