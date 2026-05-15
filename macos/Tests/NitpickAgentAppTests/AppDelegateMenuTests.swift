import AppKit
import XCTest

@testable import NitpickAgentApp
@testable import NitpickAgentMacOSCore

final class AppDelegateMenuTests: XCTestCase {
    @MainActor
    func testMenuPlacesConfigActionsFirstThenReviewsThenActivityAndRemovesQuitShortcut() throws {
        let appDelegate = AppDelegate()

        let menu = appDelegate.makeMenuForTesting()
        let quitItem = try XCTUnwrap(menu.items.last)

        let titles = menu.items.map { $0.title }
        XCTAssertEqual(titles[0], "Open Config")
        XCTAssertEqual(titles[1], "Reload Config")
        XCTAssertEqual(NSStringFromSelector(menu.items[1].action!), "reloadConfig:")
        XCTAssertTrue(menu.items[2].isSeparatorItem)
        XCTAssertTrue(menu.items[9].isSeparatorItem)
        XCTAssertEqual(titles[10], "status: starting")
        XCTAssertEqual(titles[11], "")
        XCTAssertFalse(menu.items[11].isEnabled)
        XCTAssertEqual(quitItem.title, "Quit")
        XCTAssertTrue(["quit:", "terminate:"].contains(NSStringFromSelector(quitItem.action!)))
        XCTAssertNil(quitItem.image)
        XCTAssertEqual(quitItem.keyEquivalent, "")
        XCTAssertEqual(quitItem.keyEquivalentModifierMask, [])
    }

    @MainActor
    func testStatusMenuItemDoesNotShowDiscoveryErrorsAsStatusErrors() throws {
        let appDelegate = AppDelegate()
        let menu = appDelegate.makeMenuForTesting()

        appDelegate.applyMenuSnapshotForTesting(
            MenuSnapshot(
                hostIsRunning: true,
                activityCount: 0,
                runningActivityCount: 0,
                artifactCount: 0,
                localOnlyArtifactCount: 0,
                pendingSyncArtifactCount: 0,
                reviewSourceEnabled: true,
                reviewSourceLastPollSummary: "github unavailable: failed to start GitHub CLI `gh`: No such file or directory"
            )
        )

        let statusItem = try XCTUnwrap(menu.items.first { $0.title == "status: idle" })
        XCTAssertEqual(statusItem.title, "status: idle")
        XCTAssertFalse(statusItem.isEnabled)
        XCTAssertNil(statusItem.action)
        XCTAssertNil(statusItem.image)
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
