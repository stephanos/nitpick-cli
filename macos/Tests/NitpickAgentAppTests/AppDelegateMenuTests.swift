import AppKit
import XCTest

@testable import NitpickAgentApp
@testable import NitpickAgentMacOSCore

final class AppDelegateMenuTests: XCTestCase {
    @MainActor
    func testMenuPlacesReloadConfigUnderOpenConfigAndRemovesQuitShortcut() throws {
        let appDelegate = AppDelegate()

        let menu = appDelegate.makeMenuForTesting()
        let quitItem = try XCTUnwrap(menu.items.last)

        let titles = menu.items.map { $0.title }
        XCTAssertEqual(titles[1], "Open Config")
        XCTAssertEqual(titles[2], "Reload Config")
        XCTAssertEqual(NSStringFromSelector(menu.items[2].action!), "reloadConfig:")
        XCTAssertEqual(quitItem.title, "Quit")
        XCTAssertTrue(["quit:", "terminate:"].contains(NSStringFromSelector(quitItem.action!)))
        XCTAssertNil(quitItem.image)
        XCTAssertEqual(quitItem.keyEquivalent, "")
        XCTAssertEqual(quitItem.keyEquivalentModifierMask, [])
    }

    @MainActor
    func testStatusMenuItemShowsClickableErrorState() {
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
        XCTAssertEqual(statusItem.title, "Status: Discovery error")
        XCTAssertTrue(statusItem.isEnabled)
        XCTAssertEqual(NSStringFromSelector(statusItem.action!), "showStatusDetails:")
        XCTAssertNotNil(statusItem.image)
        XCTAssertEqual(
            appDelegate.statusDetailsForTesting(),
            "github unavailable: failed to start GitHub CLI `gh`: No such file or directory"
        )
    }
}
