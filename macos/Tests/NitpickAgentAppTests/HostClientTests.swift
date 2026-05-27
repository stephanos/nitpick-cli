import Foundation
import XCTest

@testable import NitpickAgentApp

final class HostClientTests: XCTestCase {
    override func tearDown() {
        StubURLProtocol.response = nil
        StubURLProtocol.lastRequest = nil
        super.tearDown()
    }

    func testRetryFailedActivitiesSurfacesHostErrorMessage() async throws {
        let client = HostClient(baseURL: URL(string: "http://host.test")!, session: stubSession())
        StubURLProtocol.response = StubResponse(
            statusCode: 400,
            body: #"{"error":"unknown provider failure kind"}"#.data(using: .utf8)!
        )

        do {
            _ = try await client.retryFailedActivities(kind: "unknown")
            XCTFail("expected host error")
        } catch let error as HostClientError {
            XCTAssertEqual(error.localizedDescription, "unknown provider failure kind")
        }
    }

    func testRunProviderDiagnosticSurfacesHostErrorMessage() async throws {
        let client = HostClient(baseURL: URL(string: "http://host.test")!, session: stubSession())
        StubURLProtocol.response = StubResponse(
            statusCode: 400,
            body: #"{"error":"provider diagnostic checkout not found"}"#.data(using: .utf8)!
        )

        do {
            _ = try await client.runProviderDiagnostic(
                repoDir: "/missing",
                provider: "claude",
                model: nil
            )
            XCTFail("expected host error")
        } catch let error as HostClientError {
            XCTAssertEqual(error.localizedDescription, "provider diagnostic checkout not found")
        }
    }

    private func stubSession() -> URLSession {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubURLProtocol.self]
        return URLSession(configuration: configuration)
    }
}

private struct StubResponse {
    var statusCode: Int
    var body: Data
}

private final class StubURLProtocol: URLProtocol {
    nonisolated(unsafe) static var response: StubResponse?
    nonisolated(unsafe) static var lastRequest: URLRequest?

    override class func canInit(with request: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        Self.lastRequest = request
        let response = Self.response ?? StubResponse(statusCode: 200, body: Data())
        let httpResponse = HTTPURLResponse(
            url: request.url!,
            statusCode: response.statusCode,
            httpVersion: nil,
            headerFields: ["Content-Type": "application/json"]
        )!
        client?.urlProtocol(self, didReceive: httpResponse, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: response.body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}
