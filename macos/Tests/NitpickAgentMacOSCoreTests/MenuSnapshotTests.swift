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

    func testGitHubWatcherTitle() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 0,
            githubDiscoveryEnabled: true,
            githubLastPollSummary: "reviewed 1 of 1 PRs"
        )

        XCTAssertEqual(snapshot.githubTitle, "GitHub: Watching, reviewed 1 of 1 PRs")
    }

    func testRecentActivityTitlesAreLatestFirstWithRelativeTime() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 6,
            activities: [
                ActivitySnapshot(
                    id: "activity-1",
                    kind: "Review",
                    status: "Completed",
                    label: "review on org/repo#120",
                    createdAtUnix: 980,
                    updatedAtUnix: 990
                ),
                ActivitySnapshot(
                    id: "activity-2",
                    kind: "Review",
                    status: "Running",
                    label: "review on org/repo#121",
                    createdAtUnix: 995,
                    updatedAtUnix: 995
                ),
                ActivitySnapshot(
                    id: "activity-3",
                    kind: "Chat",
                    status: "Error",
                    label: nil,
                    createdAtUnix: 930,
                    updatedAtUnix: 940
                ),
                ActivitySnapshot(
                    id: "activity-4",
                    kind: "Review",
                    status: "Completed",
                    label: "review on org/repo#119",
                    createdAtUnix: 800,
                    updatedAtUnix: 800
                ),
                ActivitySnapshot(
                    id: "activity-5",
                    kind: "Review",
                    status: "Completed",
                    label: "review on org/repo#118",
                    createdAtUnix: 700,
                    updatedAtUnix: 700
                ),
                ActivitySnapshot(
                    id: "activity-6",
                    kind: "Review",
                    status: "Completed",
                    label: "review on org/repo#117",
                    createdAtUnix: 600,
                    updatedAtUnix: 600
                ),
            ],
            currentUnix: 1_000
        )

        XCTAssertEqual(snapshot.recentActivityTitles.count, 5)
        XCTAssertEqual(snapshot.recentActivityTitles[0], "5s ago   started review on org/repo#121")
        XCTAssertEqual(snapshot.recentActivityTitles[1], "10s ago  finished review on org/repo#120")
        XCTAssertEqual(snapshot.recentActivityTitles[2], "1m ago   failed chat")
    }
}
