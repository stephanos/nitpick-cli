import Foundation
import NitpickAgentMacOSCore

struct HostClient {
    var statusURL: URL
    var session: URLSession = .shared

    init(
        statusURL: URL = URL(string: "http://127.0.0.1:19783/status")!,
        session: URLSession = .shared
    ) {
        self.statusURL = statusURL
        self.session = session
    }

    func status() async throws -> HostStatus {
        let (data, response) = try await session.data(from: statusURL)
        if let response = response as? HTTPURLResponse, response.statusCode != 200 {
            throw HostClientError.unexpectedStatus(response.statusCode)
        }
        return try JSONDecoder().decode(HostStatus.self, from: data)
    }
}

enum HostClientError: Error {
    case unexpectedStatus(Int)
}
