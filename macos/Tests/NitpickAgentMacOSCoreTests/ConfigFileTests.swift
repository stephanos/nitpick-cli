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

    func testDataDirectoryUsesExplicitEnvironmentPath() {
        let dataDirectory = DataDirectory(
            environment: ["NITPICK_AGENT_DATA_DIR": "/tmp/nitpick-data"],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )

        XCTAssertEqual(dataDirectory.url.path, "/tmp/nitpick-data")
    }

    func testDataDirectoryUsesXdgDataHomeWhenPresent() {
        let dataDirectory = DataDirectory(
            environment: ["XDG_DATA_HOME": "/tmp/data"],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )

        XCTAssertEqual(dataDirectory.url.path, "/tmp/data/nitpick-agent")
    }

    func testDataDirectoryDefaultsToHomeLocalSharePath() {
        let dataDirectory = DataDirectory(
            environment: [:],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )

        XCTAssertEqual(dataDirectory.url.path, "/Users/test/.local/share/nitpick-agent")
    }

    func testDaemonLogFileLivesUnderDataDirectory() {
        let dataDirectory = DataDirectory(
            environment: ["NITPICK_AGENT_DATA_DIR": "/tmp/nitpick-data"],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )

        let logFile = DaemonLogFile(dataDirectory: dataDirectory)

        XCTAssertEqual(logFile.url.path, "/tmp/nitpick-data/logs/daemon.log")
    }
}
