import XCTest

@testable import NitpickAgentMacOSCore

final class MenuSnapshotTests: XCTestCase {
    func testStoppedHostStatusTitle() {
        let snapshot = MenuSnapshot(hostIsRunning: false, activityCount: 3)

        XCTAssertEqual(snapshot.statusTitle, "Status: Host stopped")
    }

    func testIdleStatusTitle() {
        let snapshot = MenuSnapshot(hostIsRunning: true, activityCount: 0)

        XCTAssertEqual(snapshot.statusTitle, "Status: Idle")
    }

    func testRunningStatusTitle() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 2,
            runningActivityCount: 1
        )

        XCTAssertEqual(snapshot.statusTitle, "Status: 1 running")
    }

    func testPluralActivityStatusTitle() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 2,
            runningActivityCount: 0,
            artifactCount: 5,
            localOnlyArtifactCount: 3,
            pendingSyncArtifactCount: 1
        )

        XCTAssertEqual(snapshot.statusTitle, "Status: 2 activities, 3 local, 1 pending")
    }
}
