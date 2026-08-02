import Foundation

public enum AppRoute: Hashable, Sendable {
    case today
    case briefing(date: String, edition: String, itemID: String?)
    case alert(id: String)
    case task(reference: String)

    public init?(url: URL) {
        guard url.scheme?.lowercased() == "straylight" else { return nil }
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        let host = url.host?.lowercased()
        let path = url.pathComponents.filter { $0 != "/" }

        switch host {
        case "today":
            self = .today
        case "briefing" where path.count >= 2:
            let itemID = components?.queryItems?.first(where: { $0.name == "item" })?.value
            self = .briefing(date: path[0], edition: path[1], itemID: itemID)
        case "alert" where path.count == 1:
            self = .alert(id: path[0])
        case "task" where path.count == 1:
            self = .task(reference: path[0])
        default:
            return nil
        }
    }
}
