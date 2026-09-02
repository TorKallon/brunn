import Foundation

enum LocationCredentialCapabilities {
    static let canonicalReadOnly = [
        "open",
        "query",
        "read",
        "compute",
        "verify",
        "status",
        "task.read",
    ]

    static let locationWrite = "location.write"
    static let conditionalMessageRead = "message.read"

    static var withoutMessaging: Set<String> {
        Set(canonicalReadOnly + [locationWrite])
    }

    static var withMessaging: Set<String> {
        withoutMessaging.union([conditionalMessageRead])
    }

    static func isExactAcceptedSet(_ capabilities: [String]) -> Bool {
        let actual = Set(capabilities)
        guard actual.count == capabilities.count else { return false }
        return actual == withoutMessaging || actual == withMessaging
    }
}
