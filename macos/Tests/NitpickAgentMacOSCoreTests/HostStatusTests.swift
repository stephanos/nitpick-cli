import XCTest

@testable import NitpickAgentMacOSCore

final class HostStatusTests: XCTestCase {
    func testParsesHostStatusResponse() throws {
        let input = """
        {"activity_count":2,"running_activity_count":1,"completed_activity_count":1,"error_activity_count":0,"artifact_count":5,"local_only_artifact_count":3,"pending_sync_artifact_count":1,"provider":"claude","model":null,"github_discovery_enabled":true,"github_last_poll_unix":1000,"github_last_poll_summary":"reviewed 1 of 1 PRs"}
        """.data(using: .utf8)!

        let status = try JSONDecoder().decode(HostStatus.self, from: input)

        XCTAssertEqual(status.activityCount, 2)
        XCTAssertEqual(status.runningActivityCount, 1)
        XCTAssertEqual(status.completedActivityCount, 1)
        XCTAssertEqual(status.errorActivityCount, 0)
        XCTAssertEqual(status.artifactCount, 5)
        XCTAssertEqual(status.localOnlyArtifactCount, 3)
        XCTAssertEqual(status.pendingSyncArtifactCount, 1)
        XCTAssertEqual(status.provider, "claude")
        XCTAssertNil(status.model)
        XCTAssertEqual(status.githubDiscoveryEnabled, true)
        XCTAssertEqual(status.githubLastPollUnix, 1000)
        XCTAssertEqual(status.githubLastPollSummary, "reviewed 1 of 1 PRs")
    }
}
