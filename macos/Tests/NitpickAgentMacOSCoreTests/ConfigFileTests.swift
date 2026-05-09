import XCTest

@testable import NitpickAgentMacOSCore

final class ConfigFileTests: XCTestCase {
    func testUsesExplicitConfigEnvironmentPath() {
        let config = ConfigFile(
            environment: ["NITPICK_AGENT_CONFIG": "/tmp/nitpick/config.toml"],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )

        XCTAssertEqual(config.url.path, "/tmp/nitpick/config.toml")
    }

    func testUsesXdgConfigHomeWhenPresent() {
        let config = ConfigFile(
            environment: ["XDG_CONFIG_HOME": "/tmp/config"],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )

        XCTAssertEqual(config.url.path, "/tmp/config/nitpick-agent/config.toml")
    }

    func testDefaultsToHomeConfigPath() {
        let config = ConfigFile(
            environment: [:],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )

        XCTAssertEqual(config.url.path, "/Users/test/.config/nitpick-agent/config.toml")
    }
}
