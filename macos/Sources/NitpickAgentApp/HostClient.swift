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

    func retryFailedActivities(kind: String) async throws -> RetryFailedActivitiesResult {
        var request = URLRequest(url: baseURL.appendingPathComponent("activities/retry-failed"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(RetryFailedActivitiesInput(kind: kind))
        let (data, response) = try await session.data(for: request)
        try validateActionResponse(response, data: data)
        return try JSONDecoder().decode(RetryFailedActivitiesResult.self, from: data)
    }

    func runProviderDiagnostic(
        repoDir: String,
        provider: String?,
        model: String?,
        disableSandbox: Bool = false
    ) async throws -> ActivitySnapshot {
        var request = URLRequest(url: baseURL.appendingPathComponent("debug/provider"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "repo_dir": repoDir,
            "provider": provider as Any? ?? NSNull(),
            "model": model as Any? ?? NSNull(),
            "disable_sandbox": disableSandbox,
        ])
        let (data, response) = try await session.data(for: request)
        try validateActionResponse(response, data: data)
        return try JSONDecoder().decode(ActivitySnapshot.self, from: data)
    }

    private func validateActionResponse(_ response: URLResponse, data: Data) throws {
        guard let response = response as? HTTPURLResponse, response.statusCode != 200 else {
            return
        }
        if let hostError = try? JSONDecoder().decode(HostErrorResponse.self, from: data),
           !hostError.error.isEmpty
        {
            throw HostClientError.hostError(response.statusCode, hostError.error)
        }
        throw HostClientError.unexpectedStatus(response.statusCode)
    }
}

struct RetryFailedActivitiesInput: Encodable {
    var kind: String
}

struct RetryFailedActivitiesResult: Decodable {
    var queued: Int
    var skipped: Int
    var activities: [String]
}

private struct HostErrorResponse: Decodable {
    var error: String
}

enum HostClientError: Error, LocalizedError {
    case unexpectedStatus(Int)
    case hostError(Int, String)

    var errorDescription: String? {
        switch self {
        case let .unexpectedStatus(statusCode):
            return "Unexpected host status \(statusCode)"
        case let .hostError(_, message):
            return message
        }
    }
}
