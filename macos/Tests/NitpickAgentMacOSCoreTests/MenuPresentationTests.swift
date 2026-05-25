import XCTest

@testable import NitpickAgentMacOSCore

final class MenuPresentationTests: XCTestCase {
    func testSnapshotRendersMenuPresentationEntries() {
        let snapshot = MenuSnapshot(
            hostIsRunning: true,
            activityCount: 2,
            openReviewCount: 2,
            reviewSourceEnabled: true,
            reviewSourceLastPollUnix: 940,
            activities: [
                ActivitySnapshot(
                    id: "activity-1",
                    kind: "Review",
                    status: "Running",
                    label: "review on org/repo#121",
                    createdAtUnix: 980,
                    updatedAtUnix: 990
                ),
                ActivitySnapshot(
                    id: "activity-2",
                    kind: "Discovery",
                    status: "Completed",
                    label: "review request org/repo#120",
                    createdAtUnix: 900,
                    updatedAtUnix: 930
                ),
            ],
            currentUnix: 1_000
        )

        let presentation = MenuPresentation(snapshot: snapshot)

        XCTAssertEqual(presentation.status.openReviewsTitle, "2 open reviews")
        XCTAssertEqual(presentation.status.title, "status: idle")
        XCTAssertNil(presentation.status.details)
        XCTAssertEqual(presentation.status.agentErrorItem.title, "")
        XCTAssertFalse(presentation.status.agentErrorItem.isEnabled)
        XCTAssertTrue(presentation.status.agentErrorItem.isHidden)
        XCTAssertEqual(presentation.lastDiscoveryRefresh.title, "last discovery: 1m ago")
        XCTAssertFalse(presentation.lastDiscoveryRefresh.isEnabled)
        XCTAssertFalse(presentation.lastDiscoveryRefresh.isHidden)
        XCTAssertEqual(
            presentation.ongoingReviews,
            [
                ActivityMenuEntry(
                    id: "activity-1",
                    title: "🤖 Running review on org/repo#121"
                ),
            ]
        )
        XCTAssertFalse(presentation.recentActivities.isHidden)
        XCTAssertEqual(
            presentation.recentActivities.items,
            [
                ActivityMenuEntry(
                    id: "activity-1",
                    title: "10s ago  started review on org/repo#121"
                ),
                ActivityMenuEntry(
                    id: "activity-2",
                    title: "1m ago   review request org/repo#120"
                ),
            ]
        )
    }

    func testStoppedAgentRendersHiddenDiscoveryAndVisibleAgentErrorContract() {
        let snapshot = MenuSnapshot(
            hostIsRunning: false,
            activityCount: 0,
            statusIssue: MenuStatusIssue(
                title: "status: agent error",
                details: "config: /tmp/config.toml\nlog: /tmp/daemon.log\n\nunknown field `checkout_dir`"
            )
        )

        let presentation = MenuPresentation(snapshot: snapshot)

        XCTAssertEqual(presentation.status.openReviewsTitle, "no open reviews")
        XCTAssertEqual(presentation.status.title, "status: agent error")
        XCTAssertEqual(
            presentation.status.details,
            "config: /tmp/config.toml\nlog: /tmp/daemon.log\n\nunknown field `checkout_dir`"
        )
        XCTAssertEqual(presentation.status.agentErrorItem.title, "status: agent error")
        XCTAssertTrue(presentation.status.agentErrorItem.isEnabled)
        XCTAssertFalse(presentation.status.agentErrorItem.isHidden)
        XCTAssertEqual(presentation.lastDiscoveryRefresh.title, "")
        XCTAssertFalse(presentation.lastDiscoveryRefresh.isEnabled)
        XCTAssertTrue(presentation.lastDiscoveryRefresh.isHidden)
        XCTAssertTrue(presentation.recentActivities.isHidden)
        XCTAssertEqual(presentation.recentActivities.items, [])
    }
}
