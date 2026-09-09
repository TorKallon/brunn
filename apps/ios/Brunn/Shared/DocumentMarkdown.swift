import SwiftUI

/// Keep the system Markdown parser's block tree instead of flattening long prose into Text.
/// Link filtering stays in SafeMarkdown; this view never resolves workspace paths or raw entries.
struct DocumentMarkdownNode: Identifiable {
    let id: Int
    let kind: PresentationIntent.Kind
    var text = AttributedString()
    var children: [DocumentMarkdownNode] = []

    static func parse(_ markdown: String) -> [DocumentMarkdownNode] {
        let value = SafeMarkdown.documentAttributedString(markdown)
        var nodes: [DocumentMarkdownNode] = []
        for run in value.runs {
            let path = Array((run.presentationIntent?.components ?? []).reversed())
            var text = AttributedString(value[run.range])
            text.presentationIntent = nil
            if path.isEmpty {
                nodes.append(DocumentMarkdownNode(id: -nodes.count - 1, kind: .paragraph, text: text))
            } else {
                append(text, path: path[...], to: &nodes)
            }
        }
        return nodes
    }

    private static func append(
        _ text: AttributedString,
        path: ArraySlice<PresentationIntent.IntentType>,
        to nodes: inout [DocumentMarkdownNode]
    ) {
        guard let component = path.first else { return }
        if nodes.last?.id != component.identity {
            nodes.append(DocumentMarkdownNode(id: component.identity, kind: component.kind))
        }
        let index = nodes.count - 1
        if path.count == 1 {
            nodes[index].text.append(text)
        } else {
            append(text, path: path.dropFirst(), to: &nodes[index].children)
        }
    }
}

struct DocumentMarkdown: View {
    let nodes: [DocumentMarkdownNode]

    init(markdown: String) {
        nodes = DocumentMarkdownNode.parse(markdown)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            ForEach(nodes) { DocumentMarkdownBlock(node: $0) }
        }
        .font(.body)
        .tint(BrunnTheme.signal)
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct DocumentMarkdownBlock: View {
    let node: DocumentMarkdownNode
    var orderedList = false

    var body: some View {
        switch node.kind {
        case let .header(level):
            Text(node.text)
                .font(level == 1 ? .title2.bold() : level == 2 ? .title3.bold() : .headline)
                .accessibilityAddTraits(.isHeader)
                .padding(.top, 6)
        case .orderedList, .unorderedList:
            VStack(alignment: .leading, spacing: 8) {
                ForEach(node.children) {
                    DocumentMarkdownBlock(node: $0, orderedList: node.kind == .orderedList)
                }
            }
        case let .listItem(ordinal):
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(orderedList ? "\(ordinal)." : "•")
                    .accessibilityHidden(!orderedList)
                children
            }
        case .codeBlock:
            ScrollView(.horizontal) {
                Text(node.text)
                    .font(.system(.body, design: .monospaced))
                    .fixedSize(horizontal: true, vertical: false)
                    .padding(12)
            }
            .background(.background, in: RoundedRectangle(cornerRadius: 8))
            .accessibilityLabel("Code block")
        case .blockQuote:
            children
                .padding(.leading, 12)
                .overlay(alignment: .leading) {
                    Rectangle().fill(BrunnTheme.line).frame(width: 3)
                }
        case .table:
            table
        default:
            if node.children.isEmpty {
                Text(node.text).fixedSize(horizontal: false, vertical: true)
            } else {
                children
            }
        }
    }

    private var children: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(node.children) { DocumentMarkdownBlock(node: $0) }
        }
    }

    // Labelled rows keep each column's relationship visible without tiny type or clipping
    // on an iPhone. The full cell text and links remain selectable at accessibility sizes.
    private var table: some View {
        let headers = node.children.first(where: { $0.kind == .tableHeaderRow })?.children ?? []
        let rows = node.children.filter { $0.kind != .tableHeaderRow }
        return VStack(alignment: .leading, spacing: 12) {
            ForEach(rows.isEmpty ? node.children : rows) { row in
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(row.children) { cell in
                        let index = columnIndex(cell)
                        VStack(alignment: .leading, spacing: 3) {
                            if let header = headers.first(where: { columnIndex($0) == index }), !rows.isEmpty {
                                Text(header.text).font(.subheadline.bold())
                            }
                            Text(cell.text).fixedSize(horizontal: false, vertical: true)
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
                .background(.background, in: RoundedRectangle(cornerRadius: 8))
            }
        }
    }

    private func columnIndex(_ cell: DocumentMarkdownNode) -> Int {
        if case let .tableCell(columnIndex) = cell.kind { return columnIndex }
        return 0
    }
}
