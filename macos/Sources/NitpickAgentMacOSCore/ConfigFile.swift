import Foundation

public struct ConfigFile {
    public let url: URL

    public init(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        homeDirectoryURL: URL = FileManager.default.homeDirectoryForCurrentUser
    ) {
        if let configuredPath = environment["NITPICK_AGENT_CONFIG"], !configuredPath.isEmpty {
            url = URL(fileURLWithPath: configuredPath)
            return
        }

        url = homeDirectoryURL
            .appendingPathComponent("Library")
            .appendingPathComponent("Application Support")
            .appendingPathComponent("dev.nitpick.nitpick-agent")
            .appendingPathComponent("config.toml")
    }
}

public struct DataDirectory {
    public let url: URL

    public init(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        homeDirectoryURL: URL = FileManager.default.homeDirectoryForCurrentUser
    ) {
        if let configuredPath = environment["NITPICK_AGENT_DATA_DIR"], !configuredPath.isEmpty {
            url = URL(fileURLWithPath: configuredPath)
            return
        }

        url = homeDirectoryURL
            .appendingPathComponent("Library")
            .appendingPathComponent("Application Support")
            .appendingPathComponent("dev.nitpick.nitpick-agent")
    }
}

public struct DaemonLogFile {
    public let url: URL

    public init(dataDirectory: DataDirectory = DataDirectory()) {
        url = dataDirectory.url
            .appendingPathComponent("logs")
            .appendingPathComponent("daemon.log")
    }
}
