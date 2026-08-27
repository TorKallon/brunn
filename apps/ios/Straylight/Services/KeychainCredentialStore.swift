import Foundation
import Security

@MainActor
protocol CredentialStoring: AnyObject {
    func load() throws -> DeviceTaskCredential?
    func save(_ credential: DeviceTaskCredential) throws
    func delete() throws
}

struct DeviceTaskCredential: Codable, Sendable, Equatable {
    let credentialRef: String
    let token: String
}

enum KeychainCredentialError: Error, LocalizedError {
    case unexpectedStatus(OSStatus)
    case invalidData

    var errorDescription: String? {
        switch self {
        case let .unexpectedStatus(status):
            "Keychain returned status \(status)."
        case .invalidData:
            "The saved Straylight credential is invalid."
        }
    }
}

@MainActor
final class KeychainCredentialStore: CredentialStoring {
    private let service = "com.rourkem.straylight.api"
    private let account: String
    private let removesLegacyCredential: Bool

    init() {
#if DEBUG
        if let namespace = ProcessInfo.processInfo.environment["STRAYLIGHT_CREDENTIAL_NAMESPACE"],
           !namespace.isEmpty
        {
            account = "ios-task-device-v1-\(namespace)"
            removesLegacyCredential = false
        } else {
            account = "ios-task-device-v1"
            removesLegacyCredential = true
        }
#else
        account = "ios-task-device-v1"
        removesLegacyCredential = true
#endif
    }

    func load() throws -> DeviceTaskCredential? {
        try removeLegacyCredentialIfNeeded()
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else {
            throw KeychainCredentialError.unexpectedStatus(status)
        }
        guard
            let data = result as? Data,
            let credential = try? JSONDecoder().decode(DeviceTaskCredential.self, from: data),
            !credential.credentialRef.isEmpty
        else {
            throw KeychainCredentialError.invalidData
        }
        return credential
    }

    func save(_ credential: DeviceTaskCredential) throws {
        try removeLegacyCredentialIfNeeded()
        let data = try JSONEncoder().encode(credential)
        let baseQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        let attributes: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        ]

        let updateStatus = SecItemUpdate(baseQuery as CFDictionary, attributes as CFDictionary)
        if updateStatus == errSecSuccess { return }
        guard updateStatus == errSecItemNotFound else {
            throw KeychainCredentialError.unexpectedStatus(updateStatus)
        }

        var addQuery = baseQuery
        attributes.forEach { addQuery[$0.key] = $0.value }
        let addStatus = SecItemAdd(addQuery as CFDictionary, nil)
        guard addStatus == errSecSuccess else {
            throw KeychainCredentialError.unexpectedStatus(addStatus)
        }
    }

    func delete() throws {
        try removeLegacyCredentialIfNeeded()
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw KeychainCredentialError.unexpectedStatus(status)
        }
    }

    private func removeLegacyCredentialIfNeeded() throws {
        guard removesLegacyCredential else { return }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: "owner-alpha-read-only",
        ]
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw KeychainCredentialError.unexpectedStatus(status)
        }
    }
}
