import XCTest

@testable import NitpickAgentMacOSCore

final class MenuSnapshotTests: XCTestCase {
    func testStoppedAgentStatusTitle() {
        let snapshot = MenuSnapshot(hostIsRunning: false, activityCount: 3)

        XCTAssertEqual(snapshot.statusTitle, "status: agent stopped")
    }

    func testIdleStatusTitle() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 0,
            reviewSourceEnabled: true
        )

        XCTAssertEqual(snapshot.statusTitle, "status: idle")
    }

    func testDisabledDiscoveryStatusTitle() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 0,
            reviewSourceEnabled: false
        )

        XCTAssertEqual(snapshot.statusTitle, "status: discovery disabled")
    }

    func testReviewSourceErrorDoesNotReplaceStatusTitle() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 0,
            reviewSourceEnabled: true,
            reviewSourceLastPollSummary: "github unavailable: failed to start GitHub CLI `gh`: No such file or directory"
        )

        XCTAssertEqual(snapshot.statusTitle, "status: idle")
        XCTAssertNil(snapshot.statusDetails)
    }

    func testLastDiscoveryRefreshTitle() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 0,
            reviewSourceEnabled: true,
            reviewSourceLastPollUnix: 940,
            currentUnix: 1_000
        )

        XCTAssertEqual(snapshot.lastDiscoveryRefreshTitle, "last discovery: 1m ago")
    }

    func testLastDiscoveryRefreshTitleBeforeFirstPoll() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 0,
            reviewSourceEnabled: true,
            currentUnix: 1_000
        )

        XCTAssertEqual(snapshot.lastDiscoveryRefreshTitle, "last discovery: never")
    }

    func testRunningStatusTitle() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 2,
            runningActivityCount: 1
        )

        XCTAssertEqual(snapshot.statusTitle, "status: 1 running")
    }

    func testIdleStatusIgnoresHistoricalActivityCount() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 22,
            runningActivityCount: 0,
            reviewSourceEnabled: true
        )

        XCTAssertEqual(snapshot.statusTitle, "status: idle")
    }

    func testIdleStatusIncludesArtifactSyncState() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 22,
            runningActivityCount: 0,
            artifactCount: 5,
            localOnlyArtifactCount: 3,
            pendingSyncArtifactCount: 1,
            reviewSourceEnabled: true
        )

        XCTAssertEqual(snapshot.statusTitle, "status: idle, 3 local, 1 pending")
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

        XCTAssertEqual(snapshot.recentActivityEntries.count, 5)
        XCTAssertEqual(snapshot.recentActivityEntries[0].id, "activity-2")
        XCTAssertEqual(snapshot.recentActivityEntries[0].title, "5s ago   started review on org/repo#121")
        XCTAssertEqual(snapshot.recentActivityEntries[1].title, "10s ago  finished review on org/repo#120")
        XCTAssertEqual(snapshot.recentActivityEntries[2].title, "1m ago   failed chat")
    }

    func testOngoingReviewTitlesShowRunningThenQueuedWithOverflow() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 7,
            activities: [
                ActivitySnapshot(
                    id: "activity-1",
                    kind: "Review",
                    status: "Queued",
                    label: "review on org/repo#1",
                    createdAtUnix: 900,
                    updatedAtUnix: 900
                ),
                ActivitySnapshot(
                    id: "activity-2",
                    kind: "Review",
                    status: "Running",
                    label: "review on org/repo#2",
                    createdAtUnix: 910,
                    updatedAtUnix: 910
                ),
                ActivitySnapshot(
                    id: "activity-3",
                    kind: "Review",
                    status: "Queued",
                    label: "review on org/repo#3",
                    createdAtUnix: 920,
                    updatedAtUnix: 920
                ),
                ActivitySnapshot(
                    id: "activity-4",
                    kind: "Review",
                    status: "Queued",
                    label: "review on org/repo#4",
                    createdAtUnix: 930,
                    updatedAtUnix: 930
                ),
                ActivitySnapshot(
                    id: "activity-5",
                    kind: "Review",
                    status: "Queued",
                    label: "review on org/repo#5",
                    createdAtUnix: 940,
                    updatedAtUnix: 940
                ),
                ActivitySnapshot(
                    id: "activity-6",
                    kind: "Review",
                    status: "Queued",
                    label: "review on org/repo#6",
                    createdAtUnix: 950,
                    updatedAtUnix: 950
                ),
                ActivitySnapshot(
                    id: "activity-7",
                    kind: "Chat",
                    status: "Running",
                    label: nil,
                    createdAtUnix: 960,
                    updatedAtUnix: 960
                ),
            ]
        )

        XCTAssertEqual(snapshot.ongoingReviewEntries.count, 6)
        XCTAssertEqual(snapshot.ongoingReviewEntries[0].id, "activity-2")
        XCTAssertEqual(snapshot.ongoingReviewEntries[0].title, "Running review on org/repo#2")
        XCTAssertEqual(snapshot.ongoingReviewEntries[1].title, "Queued review on org/repo#6")
        XCTAssertNil(snapshot.ongoingReviewEntries[5].id)
        XCTAssertEqual(snapshot.ongoingReviewEntries[5].title, "1 more queued...")
    }

    func testCompletedCleanupActivityUsesLabelAsEventText() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 1,
            activities: [
                ActivitySnapshot(
                    id: "activity-1",
                    kind: "Maintenance",
                    status: "Completed",
                    label: "acme/platform#42 cleaned up",
                    createdAtUnix: 990,
                    updatedAtUnix: 990
                ),
            ],
            currentUnix: 1_000
        )

        XCTAssertEqual(snapshot.recentActivityEntries[0].title, "10s ago  acme/platform#42 cleaned up")
    }

    func testDetectedReviewRequestActivityUsesLabelAsEventText() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 1,
            activities: [
                ActivitySnapshot(
                    id: "activity-1",
                    kind: "Discovery",
                    status: "Completed",
                    label: "review request acme/platform#42",
                    createdAtUnix: 990,
                    updatedAtUnix: 990
                ),
            ],
            currentUnix: 1_000
        )

        XCTAssertEqual(
            snapshot.recentActivityEntries[0].title,
            "10s ago  review request acme/platform#42"
        )
    }
}
