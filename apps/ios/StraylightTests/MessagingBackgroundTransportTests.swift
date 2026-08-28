@testable import Straylight
import Foundation
import UIKit
import XCTest

final class MessagingBackgroundTransportTests: XCTestCase {
    private let conversationID = "019f8800-0000-7000-8000-000000000001"

    func testStableTaskDescriptionIsOpaqueAndByteSensitive() throws {
        let url = try XCTUnwrap(URL(string:
            "https://straylight.example/api/v1/workspace/messaging/conversations/\(conversationID)/messages"
        ))
        let clientKey = "01J00000000000000000000000"
        let body = Data(
            #"{"client_key":"01J00000000000000000000000","kind":"text","body_md":"private exact body","refs":[]}"#.utf8
        )

        let first = MessagingBackgroundTransport.stableTaskDescription(
            requestURL: url,
            exactRequestData: body
        )
        let second = MessagingBackgroundTransport.stableTaskDescription(
            requestURL: url,
            exactRequestData: body
        )
        let changed = MessagingBackgroundTransport.stableTaskDescription(
            requestURL: url,
            exactRequestData: body + Data([0x20])
        )

        XCTAssertEqual(first, second)
        XCTAssertNotEqual(first, changed)
        XCTAssertFalse(first.contains(conversationID))
        XCTAssertFalse(first.contains(clientKey))
        XCTAssertFalse(first.contains("private exact body"))
        XCTAssertFalse(first.contains("bearer-secret"))
    }

    func testPreparedUploadFilePreservesExactBytesAndPrivacyAttributes() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let url = try XCTUnwrap(URL(string: "https://straylight.example/messages"))
        let body = Data([0x00, 0x7b, 0xff, 0x0a, 0x7d])
        let description = MessagingBackgroundTransport.stableTaskDescription(
            requestURL: url,
            exactRequestData: body
        )

        let fileURL = try MessagingBackgroundTransport.prepareUploadFile(
            exactRequestData: body,
            taskDescription: description,
            directory: root
        )

        XCTAssertEqual(try Data(contentsOf: fileURL), body)
        XCTAssertEqual(
            MessagingBackgroundTransport.fileProtection,
            .completeUntilFirstUserAuthentication
        )
        XCTAssertEqual(
            try fileURL.resourceValues(forKeys: [.isExcludedFromBackupKey])
                .isExcludedFromBackup,
            true
        )
        XCTAssertEqual(
            try root.resourceValues(forKeys: [.isExcludedFromBackupKey])
                .isExcludedFromBackup,
            true
        )
    }

    func testPreparedUploadFileRejectsChangedBytesAtStableIdentity() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let url = try XCTUnwrap(URL(string: "https://straylight.example/messages"))
        let body = Data(#"{"client_key":"01J00000000000000000000000"}"#.utf8)
        let description = MessagingBackgroundTransport.stableTaskDescription(
            requestURL: url,
            exactRequestData: body
        )
        _ = try MessagingBackgroundTransport.prepareUploadFile(
            exactRequestData: body,
            taskDescription: description,
            directory: root
        )

        XCTAssertThrowsError(try MessagingBackgroundTransport.prepareUploadFile(
            exactRequestData: Data(#"{"client_key":"01J00000000000000000000001"}"#.utf8),
            taskDescription: description,
            directory: root
        )) { error in
            XCTAssertEqual(
                error as? MessagingBackgroundTransportError,
                .persistedBodyMismatch
            )
        }
    }

    func testAuthenticatedSessionCleanupPurgesOrphanWithoutTouchingSiblingData() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let uploadDirectory = root.appendingPathComponent(
            "BackgroundUploads",
            isDirectory: true
        )
        let unrelatedFile = root.appendingPathComponent("keep.txt")
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(
            at: root,
            withIntermediateDirectories: true
        )
        try Data("unrelated".utf8).write(to: unrelatedFile)

        let body = Data(#"{"body_md":"orphaned private body"}"#.utf8)
        let url = try XCTUnwrap(URL(string: "https://straylight.example/messages"))
        let description = MessagingBackgroundTransport.stableTaskDescription(
            requestURL: url,
            exactRequestData: body
        )
        _ = try MessagingBackgroundTransport.prepareUploadFile(
            exactRequestData: body,
            taskDescription: description,
            directory: uploadDirectory
        )
        let transport = MessagingBackgroundTransport(
            fileManager: .default,
            uploadDirectory: uploadDirectory,
            configuration: .ephemeral
        )

        let purged = await transport.cancelAllAndPurgeArtifacts()

        XCTAssertTrue(purged)
        XCTAssertFalse(FileManager.default.fileExists(atPath: uploadDirectory.path))
        XCTAssertEqual(try Data(contentsOf: unrelatedFile), Data("unrelated".utf8))
    }

    func testPurgeFailureBlocksUploadsUntilACompleteRetry() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let uploadDirectory = root.appendingPathComponent(
            "BackgroundUploads",
            isDirectory: true
        )
        defer { try? FileManager.default.removeItem(at: root) }

        let body = Data(#"{"body_md":"protected pending body"}"#.utf8)
        let url = try XCTUnwrap(URL(string: "https://straylight.example/messages"))
        let description = MessagingBackgroundTransport.stableTaskDescription(
            requestURL: url,
            exactRequestData: body
        )
        _ = try MessagingBackgroundTransport.prepareUploadFile(
            exactRequestData: body,
            taskDescription: description,
            directory: uploadDirectory
        )
        let remover = PurgeFailureToggle()
        let transport = MessagingBackgroundTransport(
            fileManager: .default,
            uploadDirectory: uploadDirectory,
            configuration: .ephemeral,
            artifactRemover: { try remover.remove($0) }
        )

        let firstPurge = await transport.cancelAllAndPurgeArtifacts()
        XCTAssertFalse(firstPurge)
        XCTAssertTrue(FileManager.default.fileExists(atPath: uploadDirectory.path))

        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        do {
            _ = try await transport.upload(
                request: request,
                exactRequestData: body,
                acknowledgementTimeout: 0.1
            )
            XCTFail("Uploads must remain fail-closed while protected data could not be purged")
        } catch {
            XCTAssertEqual(
                error as? MessagingBackgroundTransportError,
                .artifactPurgeFailed
            )
        }

        remover.allowRemoval()
        let retryPurge = await transport.cancelAllAndPurgeArtifacts()
        XCTAssertTrue(retryPurge)
        XCTAssertFalse(FileManager.default.fileExists(atPath: uploadDirectory.path))
    }

    func testAuthenticatedSessionCleanupCancelsPendingUploadAndPurgesBody() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        let uploadDirectory = root.appendingPathComponent(
            "BackgroundUploads",
            isDirectory: true
        )
        defer {
            PendingMessagingUploadURLProtocol.didStart = nil
            PendingMessagingUploadURLProtocol.didStop = nil
            try? FileManager.default.removeItem(at: root)
        }

        let started = expectation(description: "pending upload started")
        let stopped = expectation(description: "pending upload cancelled")
        started.assertForOverFulfill = true
        stopped.assertForOverFulfill = true
        PendingMessagingUploadURLProtocol.didStart = { started.fulfill() }
        PendingMessagingUploadURLProtocol.didStop = { stopped.fulfill() }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [PendingMessagingUploadURLProtocol.self]
        let transport = MessagingBackgroundTransport(
            fileManager: .default,
            uploadDirectory: uploadDirectory,
            configuration: configuration
        )
        let body = Data(#"{"body_md":"pending private body"}"#.utf8)
        let url = try XCTUnwrap(URL(string: "https://straylight.example/messages"))
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        let upload = Task {
            try await transport.upload(
                request: request,
                exactRequestData: body,
                acknowledgementTimeout: 60
            )
        }

        await fulfillment(of: [started], timeout: 1)
        XCTAssertTrue(FileManager.default.fileExists(atPath: uploadDirectory.path))

        let purged = await transport.cancelAllAndPurgeArtifacts()
        await fulfillment(of: [stopped], timeout: 1)

        XCTAssertTrue(purged)
        XCTAssertFalse(FileManager.default.fileExists(atPath: uploadDirectory.path))
        do {
            _ = try await upload.value
            XCTFail("A purged authenticated upload must not complete successfully")
        } catch {
            XCTAssertTrue(error is CancellationError, "Unexpected error: \(type(of: error))")
        }
    }

    func testAppDelegateSelectsOnlyMessagingBackgroundSession() {
        XCTAssertTrue(AppDelegate.handlesBackgroundURLSession(
            identifier: MessagingBackgroundTransport.sessionIdentifier
        ))
        XCTAssertFalse(AppDelegate.handlesBackgroundURLSession(
            identifier: "com.rourkem.straylight.some-other-session"
        ))
    }

    @MainActor
    func testOwnedBackgroundSessionCompletionRunsExactlyOnceAfterEventsFinish() async {
        let completed = expectation(description: "owned session completion")
        completed.assertForOverFulfill = true

        XCTAssertTrue(MessagingBackgroundTransport.handleBackgroundEvents(
            identifier: MessagingBackgroundTransport.sessionIdentifier,
            completionHandler: { completed.fulfill() }
        ))
        MessagingBackgroundTransport.shared.urlSessionDidFinishEvents(
            forBackgroundURLSession: .shared
        )

        await fulfillment(of: [completed], timeout: 1)
        MessagingBackgroundTransport.shared.urlSessionDidFinishEvents(
            forBackgroundURLSession: .shared
        )
        await Task.yield()
    }

    @MainActor
    func testAppDelegateCompletesForeignBackgroundSessionExactlyOnce() async {
        let completed = expectation(description: "foreign session completion")
        completed.assertForOverFulfill = true
        let delegate = AppDelegate()

        delegate.application(
            UIApplication.shared,
            handleEventsForBackgroundURLSession: "com.rourkem.straylight.foreign",
            completionHandler: {
                completed.fulfill()
            }
        )

        await fulfillment(of: [completed], timeout: 1)
    }
}

private final class PurgeFailureToggle: @unchecked Sendable {
    private let lock = NSLock()
    private var removalAllowed = false

    func allowRemoval() {
        lock.withLock { removalAllowed = true }
    }

    func remove(_ url: URL) throws {
        let allowed = lock.withLock { removalAllowed }
        guard allowed else { throw CocoaError(.fileWriteNoPermission) }
        try FileManager.default.removeItem(at: url)
    }
}

private final class PendingMessagingUploadURLProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var didStart: (@Sendable () -> Void)?
    nonisolated(unsafe) static var didStop: (@Sendable () -> Void)?

    override class func canInit(with _: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.didStart?()
    }

    override func stopLoading() {
        Self.didStop?()
    }
}
