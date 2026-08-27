import Foundation

public enum AppRoute: Hashable, Sendable {
    case today
    case briefing(date: String, edition: String, itemID: String?)
    case notification(notificationRef: String, deliveryRef: String?)
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
        case "notification" where path.count == 1:
            guard let notificationRef = PushReference.notification(rawID: path[0]) else {
                return nil
            }
            let rawDelivery = components?.queryItems?
                .first(where: { $0.name == "delivery" })?
                .value
            let deliveryRef: String?
            if let rawDelivery {
                guard let parsed = PushReference.delivery(rawID: rawDelivery) else {
                    return nil
                }
                deliveryRef = parsed
            } else {
                deliveryRef = nil
            }
            self = .notification(
                notificationRef: notificationRef,
                deliveryRef: deliveryRef
            )
        case "task" where path.count == 1:
            guard let reference = TaskReference.canonical(path[0]) else { return nil }
            self = .task(reference: reference)
        default:
            return nil
        }
    }
}

public enum TaskReference {
    public static func canonical(_ value: String) -> String? {
        guard value.count == 36,
              value == value.lowercased(),
              value.utf8.enumerated().allSatisfy({ index, byte in
                  if [8, 13, 18, 23].contains(index) { return byte == 45 }
                  return (byte >= 48 && byte <= 57) || (byte >= 97 && byte <= 102)
              }),
              value[value.index(value.startIndex, offsetBy: 14)] == "7",
              "89ab".contains(value[value.index(value.startIndex, offsetBy: 19)]),
              UUID(uuidString: value) != nil
        else { return nil }
        return value
    }
}

public enum PushReference {
    public static func notification(rawID: String) -> String? {
        prefixed(rawID: rawID, prefix: "notification")
    }

    public static func delivery(rawID: String) -> String? {
        prefixed(rawID: rawID, prefix: "delivery")
    }

    public static func isNotification(_ reference: String) -> Bool {
        rawID(reference: reference, prefix: "notification") != nil
    }

    public static func isDelivery(_ reference: String) -> Bool {
        rawID(reference: reference, prefix: "delivery") != nil
    }

    public static func rawNotificationID(_ reference: String) -> String? {
        rawID(reference: reference, prefix: "notification")
    }

    public static func rawDeliveryID(_ reference: String) -> String? {
        rawID(reference: reference, prefix: "delivery")
    }

    private static func prefixed(rawID: String, prefix: String) -> String? {
        guard isLowercaseHexID(rawID) else { return nil }
        return "\(prefix):\(rawID)"
    }

    private static func rawID(reference: String, prefix: String) -> String? {
        let expectedPrefix = "\(prefix):"
        guard reference.hasPrefix(expectedPrefix) else { return nil }
        let value = String(reference.dropFirst(expectedPrefix.count))
        return isLowercaseHexID(value) ? value : nil
    }

    private static func isLowercaseHexID(_ value: String) -> Bool {
        value.count == 32 && value.utf8.allSatisfy {
            ($0 >= 48 && $0 <= 57) || ($0 >= 97 && $0 <= 102)
        }
    }
}
