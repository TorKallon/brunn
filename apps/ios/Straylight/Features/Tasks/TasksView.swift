import SwiftUI

struct TasksView: View {
    @EnvironmentObject private var model: AppModel
    @State private var selection: TaskSegment = .next

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                VStack(alignment: .leading, spacing: 6) {
                    Eyebrow(text: "Agent-first work")
                    Text("Tasks")
                        .font(.largeTitle.bold())
                    Text(model.isDemo
                        ? "In-memory examples only · no task state is persisted"
                        : "Hosted Straylight remains the sole authoritative task store")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }

                if model.isDemo {
                    Picker("Task view", selection: $selection) {
                        ForEach(TaskSegment.allCases) { segment in
                            Text(segment.title).tag(segment)
                        }
                    }
                    .pickerStyle(.segmented)

                    VStack(alignment: .leading, spacing: 8) {
                        HStack {
                            Text(selection.heading)
                                .font(.title3.bold())
                            Spacer()
                            Text("\(visibleTasks.count)")
                                .font(.subheadline.monospacedDigit())
                                .foregroundStyle(.secondary)
                        }
                        ForEach(visibleTasks) { task in
                            NavigationLink {
                                TaskDetailView(task: task)
                            } label: {
                                TaskRow(task: task)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                } else {
                    BoundaryNotice(
                        symbol: "checklist",
                        title: "The task projection is the next server boundary",
                        detail: "Straylight currently stores tasks as canonical Markdown conventions, "
                            + "but it does not expose revision-safe Next, Inbox, Waiting, completion, "
                            + "or defer operations. The app will not parse and mutate arbitrary notes independently."
                    )

                    Button {
                        model.selectedTab = .today
                        Task { await model.performSearch("TODO tasks next waiting backlog") }
                    } label: {
                        Label("Find tracked task sources", systemImage: "magnifyingglass")
                            .frame(maxWidth: .infinity, minHeight: 44)
                    }
                    .buttonStyle(.bordered)
                }
            }
            .padding(16)
            .frame(maxWidth: 720)
            .frame(maxWidth: .infinity)
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("Tasks")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) { BrandMark() }
        }
    }

    private var visibleTasks: [TaskItem] {
        switch selection {
        case .next:
            model.tasks.filter { $0.state == .open || $0.state == .active }
        case .inbox:
            []
        case .waiting:
            model.tasks.filter { $0.state == .waiting }
        }
    }
}

private enum TaskSegment: String, CaseIterable, Identifiable {
    case next
    case inbox
    case waiting

    var id: String {
        rawValue
    }

    var title: String {
        rawValue.capitalized
    }

    var heading: String {
        switch self {
        case .next: "Do next"
        case .inbox: "Clarify"
        case .waiting: "Waiting"
        }
    }
}

private struct TaskRow: View {
    let task: TaskItem

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: task.state == .waiting ? "clock" : "circle")
                .font(.title3)
                .foregroundStyle(task.state == .waiting ? StraylightTheme.amber : StraylightTheme.forest)
                .frame(width: 30, height: 30)
            VStack(alignment: .leading, spacing: 6) {
                Text(task.title)
                    .font(.headline)
                    .foregroundStyle(StraylightTheme.ink)
                if let reason = task.reason {
                    Text(reason)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                HStack(spacing: 8) {
                    if let context = task.context {
                        StatusPill(text: context, color: StraylightTheme.blue)
                    }
                    if let minutes = task.estimatedMinutes {
                        Label("\(minutes)m", systemImage: "clock")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            Spacer(minLength: 4)
            Image(systemName: "chevron.right")
                .font(.caption.weight(.bold))
                .foregroundStyle(.tertiary)
                .padding(.top, 6)
        }
        .padding(14)
        .background(.background, in: RoundedRectangle(cornerRadius: 8))
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .stroke(StraylightTheme.line, lineWidth: 1)
        }
    }
}

private struct TaskDetailView: View {
    let task: TaskItem

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                StatusPill(
                    text: task.state.rawValue.capitalized,
                    color: task.state == .waiting ? StraylightTheme.amber : StraylightTheme.forest
                )
                Text(task.title)
                    .font(.largeTitle.bold())
                if let note = task.note {
                    Text(note)
                        .font(.body)
                }
                if let reason = task.reason {
                    LabeledContent("Why surfaced") {
                        Text(reason).multilineTextAlignment(.trailing)
                    }
                }
                Divider()
                Text("Demo only. Completion, reopen, and defer remain disabled until the server exposes revision-guarded task mutations.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            .padding(16)
            .frame(maxWidth: 720)
            .frame(maxWidth: .infinity)
        }
        .background(StraylightTheme.canvas)
        .navigationTitle("Task")
        .navigationBarTitleDisplayMode(.inline)
    }
}
