import AppKit
import XCTest

@testable import NitpickAgentApp
@testable import NitpickAgentMacOSCore

final class AppDelegateMenuTests: XCTestCase {
    @MainActor
    func testMenuPlacesConfigActionsBelowActivityRowsAndRemovesQuitShortcut() throws {
        let appDelegate = AppDelegate()

        let menu = appDelegate.makeMenuForTesting()
        let quitItem = try XCTUnwrap(menu.items.last)

        let titles = menu.items.map { $0.title }
        XCTAssertEqual(titles[1], "")
        XCTAssertFalse(menu.items[1].isEnabled)
        let openConfigIndex = try XCTUnwrap(titles.firstIndex(of: "Open Config"))
        XCTAssertGreaterThan(openConfigIndex, 12)
        XCTAssertEqual(titles[openConfigIndex + 1], "Reload Config")
        XCTAssertEqual(NSStringFromSelector(menu.items[openConfigIndex + 1].action!), "reloadConfig:")
        XCTAssertEqual(quitItem.title, "Quit")
        XCTAssertTrue(["quit:", "terminate:"].contains(NSStringFromSelector(quitItem.action!)))
        XCTAssertNil(quitItem.image)
        XCTAssertEqual(quitItem.keyEquivalent, "")
        XCTAssertEqual(quitItem.keyEquivalentModifierMask, [])
    }

    @MainActor
    func testStatusMenuItemDoesNotShowDiscoveryErrorsAsStatusErrors() {
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

        let statusItem = menu.items[0]
        XCTAssertEqual(statusItem.title, "status: idle")
        XCTAssertFalse(statusItem.isEnabled)
        XCTAssertNil(statusItem.action)
        XCTAssertNil(statusItem.image)
        XCTAssertNil(appDelegate.statusDetailsForTesting())
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
