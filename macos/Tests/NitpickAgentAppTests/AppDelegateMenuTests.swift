import AppKit
import XCTest

@testable import NitpickAgentApp
@testable import NitpickAgentMacOSCore

final class AppDelegateMenuTests: XCTestCase {
    @MainActor
    func testMenuPlacesConfigActionsFirstThenActivityAndRemovesQuitShortcut() throws {
        let appDelegate = AppDelegate()

        let menu = appDelegate.makeMenuForTesting()
        let quitItem = try XCTUnwrap(menu.items.last)

        appDelegate.applyMenuSnapshotForTesting(
            MenuSnapshot(
                hostIsRunning: true,
                activityCount: 0,
                reviewSourceEnabled: true
            )
        )

        let titles = menu.items.map { $0.title }
        XCTAssertEqual(titles[0], "Open Config")
        XCTAssertEqual(titles[1], "Reload Config")
        XCTAssertEqual(NSStringFromSelector(menu.items[1].action!), "reloadConfig:")
        XCTAssertTrue(menu.items[2].isSeparatorItem)
        XCTAssertTrue(menu.items[3].isHidden)
        XCTAssertTrue(menu.items[4].isHidden)
        XCTAssertTrue(menu.items[11].isHidden)
        XCTAssertEqual(titles[12], "Activity Log")
        XCTAssertFalse(menu.items[12].isEnabled)
        XCTAssertNotNil(menu.items[12].attributedTitle)
        XCTAssertFalse(titles.contains("status: idle"))
        XCTAssertEqual(titles[13], "last discovery: never")
        XCTAssertFalse(menu.items[13].isEnabled)
        XCTAssertEqual(quitItem.title, "Quit")
        XCTAssertTrue(["quit:", "terminate:"].contains(NSStringFromSelector(quitItem.action!)))
        XCTAssertNil(quitItem.image)
        XCTAssertEqual(quitItem.keyEquivalent, "")
        XCTAssertEqual(quitItem.keyEquivalentModifierMask, [])
    }

    @MainActor
    func testReviewsSectionIsVisibleOnlyWithActiveReviews() throws {
        let appDelegate = AppDelegate()
        let menu = appDelegate.makeMenuForTesting()

        appDelegate.applyMenuSnapshotForTesting(
            MenuSnapshot(
                hostIsRunning: true,
                activityCount: 1,
                reviewSourceEnabled: true,
                activities: [
                    ActivitySnapshot(
                        id: "activity-1",
                        kind: "Review",
                        status: "Running",
                        label: "review on org/repo#1",
                        createdAtUnix: 1_000,
                        updatedAtUnix: 1_000
                    ),
                ]
            )
        )

        XCTAssertEqual(menu.items[4].title, "Reviews")
        XCTAssertFalse(menu.items[4].isHidden)
        XCTAssertFalse(menu.items[5].isHidden)
        XCTAssertEqual(menu.items[5].title, "Running review on org/repo#1")
        XCTAssertFalse(menu.items[11].isHidden)
    }

    @MainActor
    func testDiscoveryErrorsDoNotShowAsAgentError() throws {
        let appDelegate = AppDelegate()
        let menu = appDelegate.makeMenuForTesting()

        appDelegate.applyMenuSnapshotForTesting(
            MenuSnapshot(
                hostIsRunning: true,
                activityCount: 0,
                reviewSourceEnabled: true,
                reviewSourceLastPollSummary: "github unavailable: failed to start GitHub CLI `gh`: No such file or directory"
            )
        )

        XCTAssertTrue(menu.items[3].isHidden)
        XCTAssertNil(appDelegate.statusDetailsForTesting())
    }

    @MainActor
    func testStatusMenuItemShowsClickableAgentError() throws {
        let appDelegate = AppDelegate()
        let menu = appDelegate.makeMenuForTesting()

        appDelegate.applyMenuSnapshotForTesting(
            MenuSnapshot(
                hostIsRunning: false,
                activityCount: 0,
                statusIssue: MenuStatusIssue(
                    title: "status: agent error",
                    details: "config: /tmp/config.toml\nlog: /tmp/daemon.log\n\nunknown field `checkout_dir`"
                )
            )
        )

        let statusItem = try XCTUnwrap(menu.items.first { $0.title == "status: agent error" })
        XCTAssertEqual(statusItem.title, "status: agent error")
        XCTAssertTrue(statusItem.isEnabled)
        XCTAssertEqual(NSStringFromSelector(statusItem.action!), "showStatusDetails:")
        XCTAssertNotNil(statusItem.image)
        XCTAssertEqual(
            appDelegate.statusDetailsForTesting(),
            "config: /tmp/config.toml\nlog: /tmp/daemon.log\n\nunknown field `checkout_dir`"
        )
    }

    @MainActor
    func testStoppedAgentShowsDaemonLogAsAgentError() throws {
        let appDelegate = AppDelegate()
        let menu = appDelegate.makeMenuForTesting()
        appDelegate.setDaemonLogContentsForTesting("TOML parse error at line 2, column 1\n  |\n2 | checkout_dir = \"bad\"\n  | ^ unknown field `checkout_dir`\n")

        appDelegate.applyStoppedAgentForTesting()

        let statusItem = try XCTUnwrap(menu.items.first { $0.title == "status: agent error" })
        XCTAssertEqual(statusItem.title, "status: agent error")
        XCTAssertTrue(statusItem.isEnabled)
        XCTAssertEqual(NSStringFromSelector(statusItem.action!), "showStatusDetails:")
        XCTAssertTrue(appDelegate.statusDetailsForTesting()?.contains("config: ") == true)
        XCTAssertTrue(appDelegate.statusDetailsForTesting()?.contains("log: ") == true)
        XCTAssertTrue(appDelegate.statusDetailsForTesting()?.contains("unknown field `checkout_dir`") == true)
    }

    @MainActor
    func testActivityRowsAreClickable() throws {
        let appDelegate = AppDelegate()
        let menu = appDelegate.makeMenuForTesting()

        appDelegate.setActivitiesForTesting([
            ActivitySnapshot(
                id: "activity-1",
                kind: "Discovery",
                status: "Error",
                label: "discovery poll",
                error: "failed to start GitHub CLI `gh`",
                createdAtUnix: 1_000,
                updatedAtUnix: 1_000
            ),
        ])

        let item = try XCTUnwrap(menu.items.first { item in
            item.title.contains("failed discovery poll")
        })
        XCTAssertTrue(item.isEnabled)
        XCTAssertEqual(item.representedObject as? String, "activity-1")
        XCTAssertEqual(NSStringFromSelector(item.action!), "showActivityDetails:")
    }
}
