import Foundation

extension SampleData {
    static func document(_ link: DocumentLink) throws -> PublishedDocument {
        guard link.slug == "demo-handoff", link.version == nil || (1 ... 2).contains(link.version!) else {
            throw BrunnAPIError.server(status: 404, code: "document_not_found", message: "Demo document unavailable")
        }
        let version = link.version ?? 2
        return PublishedDocument(
            slug: link.slug,
            title: "Demo published handoff",
            summary: "Demo content · not a live document. A long-form reader with explicit revisions.",
            bodyMD: """
            ## The plan

            This is **version \(version)** of the demo handoff. Links always open an authenticated published document.

            1. Read the full plan.
            2. Keep the useful details.
               - Preserve nested lists and **emphasis**.
               - Check the [external source](https://example.com/reference).

            ## Decisions

            | Topic | Earlier | Current |
            | --- | --- | --- |
            | Reading | A short preview | The entire handoff stays readable without clipping, even with larger text. |
            | Sharing | A web link | Use the stable app link by default; pin a revision explicitly. |

            ## Example code

            ```swift
            let destination = "brunn://document/demo-handoff"
            // Long code remains horizontally scrollable without truncating the source.
            ```

            > A pinned version never silently opens a different revision.

            ## Final detail

            Final detail: the complete handoff is preserved.

            [Open pinned version one](brunn://document/demo-handoff?version=1)

            [Existing dated briefing](brunn://briefing/2026-09-09/morning)

            [Private source](entry:01a08731-b88f-7711-93e2-5b3e1b33baa3) stays non-tappable.
            """,
            sources: [
                PublishedDocumentSource(label: "External reference", entryRef: nil, url: "https://example.com/reference"),
                PublishedDocumentSource(label: "Private source record", entryRef: "entry:demo", url: nil),
            ],
            version: version, currentVersion: 2,
            publishedAt: "2026-09-09T12:00:00Z", updatedAt: "2026-09-09T13:00:00Z",
            versions: [1, 2].map {
                PublishedDocumentVersion(version: $0, createdAt: "2026-09-09T12:00:00Z",
                    versionURL: "https://brunn.example/documents/demo-handoff?version=\($0)",
                    appVersionURL: DocumentLink(slug: link.slug, version: $0)!.url.absoluteString)
            },
            url: "https://brunn.example/documents/demo-handoff",
            versionURL: "https://brunn.example/documents/demo-handoff?version=\(version)",
            appURL: link.latest.url.absoluteString,
            appVersionURL: DocumentLink(slug: link.slug, version: version)!.url.absoluteString
        )
    }
}
