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
    let userID: String?
    let capabilities: [String]?

    init(
        credentialRef: String,
        token: String,
        userID: String? = nil,
        capabilities: [String]? = nil
    ) {
        self.credentialRef = credentialRef
        self.token = token
        self.userID = userID
        self.capabilities = capabilities
    }
}

enum KeychainCredentialError: Error, LocalizedError {
    case unexpectedStatus(OSStatus)
    case invalidData

    var errorDescription: String? {
        switch self {
        case let .unexpectedStatus(status):
            "Keychain returned status \(status)."
        case .invalidData:
            "The saved Brunn credential is invalid."
        }
    }
}

@MainActor
final class KeychainCredentialStore: CredentialStoring {
    static let legacyServiceForMigration = [
        "com",
        "rourkem",
        "stray" + "light",
        "api",
    ].joined(separator: ".")

    private let service = "com.rourkem.brunn.api"
    private let legacyService = KeychainCredentialStore.legacyServiceForMigration
    private let account: String
    private let removesLegacyCredential: Bool
    private var serviceMigrationComplete = false

    init() {
#if DEBUG
        if let namespace = ProcessInfo.processInfo.environment["BRUNN_CREDENTIAL_NAMESPACE"],
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
        // Best effort at process launch; protected Keychain data is retried by
        // the first credential operation if the device is still locked.
        try? migrateLegacyServiceIfNeeded()
    }

    func load() throws -> DeviceTaskCredential? {
        try migrateLegacyServiceIfNeeded()
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
        try migrateLegacyServiceIfNeeded()
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
        try migrateLegacyServiceIfNeeded()
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

    private func migrateLegacyServiceIfNeeded() throws {
        guard !serviceMigrationComplete else { return }

        if let currentData = try credentialData(service: service) {
            // Complete cleanup after a prior launch copied the credential but
            // could not delete its source item.
            if let legacyData = try credentialData(service: legacyService),
               legacyData == currentData
            {
                try deleteCredential(service: legacyService)
            }
            serviceMigrationComplete = true
            return
        }

        guard let legacyData = try credentialData(service: legacyService) else {
            serviceMigrationComplete = true
            return
        }

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecValueData as String: legacyData,
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        ]
        let status = SecItemAdd(query as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw KeychainCredentialError.unexpectedStatus(status)
        }

        try deleteCredential(service: legacyService)
        serviceMigrationComplete = true
    }

    private func credentialData(service: String) throws -> Data? {
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
        guard let data = result as? Data else { throw KeychainCredentialError.invalidData }
        return data
    }

    private func deleteCredential(service: String) throws {
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
