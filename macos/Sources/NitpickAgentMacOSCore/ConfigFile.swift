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
