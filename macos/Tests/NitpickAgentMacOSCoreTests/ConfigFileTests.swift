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

    func testDefaultsToHomeConfigPath() {
        let config = ConfigFile(
            environment: [:],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )

        XCTAssertEqual(
            config.url.path,
            "/Users/test/Library/Application Support/dev.nitpick.nitpick-agent/config.toml"
        )
    }

    func testDataDirectoryUsesExplicitEnvironmentPath() {
        let dataDirectory = DataDirectory(
            environment: ["NITPICK_AGENT_DATA_DIR": "/tmp/nitpick-data"],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )

        XCTAssertEqual(dataDirectory.url.path, "/tmp/nitpick-data")
    }

    func testDataDirectoryDefaultsToApplicationSupportPath() {
        let dataDirectory = DataDirectory(
            environment: [:],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )

        XCTAssertEqual(
            dataDirectory.url.path,
            "/Users/test/Library/Application Support/dev.nitpick.nitpick-agent"
        )
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
