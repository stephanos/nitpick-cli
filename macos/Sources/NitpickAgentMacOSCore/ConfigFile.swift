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

        if let configHome = environment["XDG_CONFIG_HOME"], !configHome.isEmpty {
            url = URL(fileURLWithPath: configHome)
                .appendingPathComponent("nitpick-agent")
                .appendingPathComponent("config.toml")
            return
        }

        url = homeDirectoryURL
            .appendingPathComponent(".config")
            .appendingPathComponent("nitpick-agent")
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

        if let dataHome = environment["XDG_DATA_HOME"], !dataHome.isEmpty {
            url = URL(fileURLWithPath: dataHome)
                .appendingPathComponent("nitpick-agent")
            return
        }

        url = homeDirectoryURL
            .appendingPathComponent(".local")
            .appendingPathComponent("share")
            .appendingPathComponent("nitpick-agent")
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
