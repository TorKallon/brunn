import Foundation
import Security

struct LocationDeviceCredential: Codable, Sendable, Equatable {
    let credentialRef: String
    let token: String
    let userID: String
    let capabilities: [String]
}

@MainActor
final class LocationKeychainCredentialStore {
    private let service = "com.rourkem.brunn.api"
    private let account: String

    init(account: String? = nil) {
        if let account {
            self.account = account
            return
        }
#if DEBUG
        if let namespace = ProcessInfo.processInfo.environment["BRUNN_CREDENTIAL_NAMESPACE"],
           !namespace.isEmpty
        {
            self.account = "ios-location-device-v1-\(namespace)"
        } else {
            self.account = "ios-location-device-v1"
        }
#else
        self.account = "ios-location-device-v1"
#endif
    }

    func load() throws -> LocationDeviceCredential? {
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
        guard let data = result as? Data,
              let credential = try? JSONDecoder().decode(
                  LocationDeviceCredential.self,
                  from: data
              ),
              credential.credentialRef.hasPrefix("credential:"),
              !credential.token.isEmpty,
              !credential.userID.isEmpty,
              LocationCredentialCapabilities.isExactAcceptedSet(credential.capabilities)
        else {
            throw KeychainCredentialError.invalidData
        }
        return credential
    }

    func save(_ credential: LocationDeviceCredential) throws {
        guard credential.credentialRef.hasPrefix("credential:"),
              !credential.token.isEmpty,
              !credential.userID.isEmpty,
              LocationCredentialCapabilities.isExactAcceptedSet(credential.capabilities)
        else {
            throw KeychainCredentialError.invalidData
        }
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
}
