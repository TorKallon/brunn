import SwiftUI

struct DocumentReaderView: View {
    @EnvironmentObject private var model: AppModel
    @State private var copiedLink = false

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    if let request = model.documentRequest {
                        switch model.documentState {
                        case .loading:
                            ProgressView("Loading document…")
                                .frame(maxWidth: .infinity, minHeight: 200)
                                .accessibilityIdentifier("document-loading")
                        case let .loaded(document):
                            content(document, request: request)
                        case .authenticationRequired:
                            BoundaryNotice(symbol: "lock", title: "Sign in to read this document",
                                detail: "Your link is saved. Sign in to Brunn to open the requested document.")
                            Button("Sign in", action: model.signInForDocument)
                                .buttonStyle(.borderedProminent)
                                .accessibilityIdentifier("document-sign-in")
                        case .unavailable:
                            BoundaryNotice(symbol: "doc.questionmark", title: "Document unavailable",
                                detail: request.version == nil
                                    ? "This document is not published or is not available to your account."
                                    : "Version \(request.version!) is not available to your account. No other revision was opened.")
                            retry
                        case let .failed(message):
                            BoundaryNotice(symbol: "exclamationmark.icloud", title: "Unable to load document", detail: message)
                            retry
                        }
                    }
                }
                .padding(16)
                .frame(maxWidth: 720)
                .frame(maxWidth: .infinity, alignment: .center)
            }
            .background(BrunnTheme.canvas)
            .navigationTitle("Document")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done", action: model.dismissDocument)
                        .accessibilityIdentifier("document-dismiss")
                }
                if case let .loaded(document) = model.documentState,
                   let request = model.documentRequest {
                    ToolbarItem(placement: .primaryAction) {
                        Menu {
                            Button(copiedLink ? "App link copied" : "Copy app link", systemImage: "link") {
                                UIPasteboard.general.url = request.latest.url
                                copiedLink = true
                                UIAccessibility.post(notification: .announcement, argument: "App link copied")
                            }
                            .accessibilityIdentifier("document-copy-app-link")
                            ShareLink(item: request.latest.url) {
                                Label("Share app link", systemImage: "square.and.arrow.up")
                            }
                            if let pinned = DocumentLink(slug: request.slug, version: document.version) {
                                ShareLink(item: pinned.url) {
                                    Label("Share pinned version \(document.version)", systemImage: "pin")
                                }
                            }
                        } label: {
                            Label("Document links", systemImage: "square.and.arrow.up")
                        }
                        .accessibilityIdentifier("document-share-menu")
                    }
                }
            }
        }
        .onChange(of: model.documentRequest) { _, _ in copiedLink = false }
        .environment(\.openURL, OpenURLAction { url in
            if SafeMarkdown.externalURL(url) { return .systemAction }
            guard SafeMarkdown.documentURL(url), let route = AppRoute(url: url) else { return .discarded }
            Task { await model.handle(route) }
            return .handled
        })
        .accessibilityIdentifier("document-reader")
    }

    private var retry: some View {
        Button("Try again") { Task { await model.retryDocument() } }
            .buttonStyle(.bordered)
            .accessibilityIdentifier("document-retry")
    }

    @ViewBuilder
    private func content(_ document: PublishedDocument, request: DocumentLink) -> some View {
        Text(document.title)
            .font(.largeTitle.bold())
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityAddTraits(.isHeader)
            .accessibilityIdentifier("document-title")

        if document.publishedAt != nil || (document.version > 1 && document.updatedAt != nil) {
            VStack(alignment: .leading, spacing: 4) {
                if let publishedAt = document.publishedAt {
                    Text("Published \(DisplayDate.metadata(publishedAt))")
                }
                if document.version > 1, let updatedAt = document.updatedAt {
                    Text("Updated \(DisplayDate.metadata(updatedAt)) · version \(document.version)")
                }
            }
            .font(.caption)
            .foregroundStyle(BrunnTheme.ink)
        }

        if request.version != nil {
            VStack(alignment: .leading, spacing: 8) {
                Text(document.version == document.currentVersion
                    ? "Pinned version \(document.version) · current at time of opening"
                    : "Historical version \(document.version) of \(document.currentVersion)")
                    .font(.subheadline.weight(.semibold))
                    .accessibilityIdentifier("document-version-notice")
                Button("Open latest") {
                    Task { await model.handle(.document(request.latest)) }
                }
                .accessibilityIdentifier("document-open-latest")
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.background, in: RoundedRectangle(cornerRadius: 8))
        }

        if let summary = document.summary, !summary.isEmpty {
            Text(summary).font(.body).textSelection(.enabled)
        }

        DocumentMarkdown(markdown: document.bodyMD)
            .id(request)

        if !document.sources.isEmpty {
            Divider()
            Text("Sources").font(.title2.bold()).accessibilityAddTraits(.isHeader)
            ForEach(Array(document.sources.enumerated()), id: \.offset) { index, source in
                if let raw = source.url, let url = URL(string: raw), SafeMarkdown.externalURL(url) {
                    Link(destination: url) {
                        Label(source.label, systemImage: "arrow.up.right.square")
                    }
                    .accessibilityHint("Opens an external source")
                    .accessibilityIdentifier("document-external-source-\(index)")
                } else {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(source.label)
                        if source.entryRef != nil {
                            Text("Private workspace reference · no document link")
                                .font(.caption).foregroundStyle(BrunnTheme.ink)
                        }
                    }
                    .accessibilityElement(children: .combine)
                }
            }
        }
    }
}
