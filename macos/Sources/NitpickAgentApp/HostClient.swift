import Foundation
import NitpickAgentMacOSCore

struct HostClient {
    var baseURL: URL
    var session: URLSession = .shared

    init(
        baseURL: URL = URL(string: "http://127.0.0.1:19783")!,
        session: URLSession = .shared
    ) {
        self.baseURL = baseURL
        self.session = session
    }

    func status() async throws -> HostStatus {
        let (data, response) = try await session.data(from: baseURL.appendingPathComponent("status"))
        if let response = response as? HTTPURLResponse, response.statusCode != 200 {
            throw HostClientError.unexpectedStatus(response.statusCode)
        }
        return try JSONDecoder().decode(HostStatus.self, from: data)
    }

    func activities() async throws -> [ActivitySnapshot] {
        let (data, response) = try await session.data(from: baseURL.appendingPathComponent("activities"))
        if let response = response as? HTTPURLResponse, response.statusCode != 200 {
            throw HostClientError.unexpectedStatus(response.statusCode)
        }
        return try JSONDecoder().decode([ActivitySnapshot].self, from: data)
    }
}

enum HostClientError: Error {
    case unexpectedStatus(Int)
}
