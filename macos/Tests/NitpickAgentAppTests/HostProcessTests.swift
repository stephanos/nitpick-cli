import XCTest

@testable import NitpickAgentApp

final class HostProcessTests: XCTestCase {
    func testCommandLookupPathKeepsConfiguredPathFirst() {
        let path = HostProcessEnvironment.commandLookupPath(
            environment: ["PATH": "/custom/bin:/usr/bin"],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )

        XCTAssertTrue(path.hasPrefix("/custom/bin:/usr/bin:/Users/test/.local/bin"))
    }

    func testCommandLookupPathIncludesUserAndHomebrewBins() {
        let path = HostProcessEnvironment.commandLookupPath(
            environment: [:],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )
        let paths = path.split(separator: ":").map(String.init)

        XCTAssertTrue(paths.contains("/Users/test/.local/bin"))
        XCTAssertTrue(paths.contains("/Users/test/bin"))
        XCTAssertTrue(paths.contains("/opt/homebrew/bin"))
        XCTAssertTrue(paths.contains("/usr/local/bin"))
    }

    func testCommandLookupPathDeduplicatesEntries() {
        let path = HostProcessEnvironment.commandLookupPath(
            environment: ["PATH": "/usr/bin:/bin:/usr/bin"],
            homeDirectoryURL: URL(fileURLWithPath: "/Users/test")
        )
        let paths = path.split(separator: ":").map(String.init)

        XCTAssertEqual(paths.filter { $0 == "/usr/bin" }.count, 1)
        XCTAssertEqual(paths.filter { $0 == "/bin" }.count, 1)
        XCTAssertEqual(paths.filter { $0 == "/opt/homebrew/bin" }.count, 1)
    }
}
