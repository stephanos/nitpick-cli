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
                openReviewCount: 3,
                reviewSourceEnabled: true
            )
        )

        let titles = menu.items.map { $0.title }
        XCTAssertEqual(titles[0], "Open Config")
        XCTAssertEqual(titles[1], "Reload Config")
        XCTAssertEqual(NSStringFromSelector(menu.items[1].action!), "reloadConfig:")
        XCTAssertTrue(menu.items[2].isSeparatorItem)
        XCTAssertEqual(titles[3], "Status")
        XCTAssertFalse(menu.items[3].isEnabled)
        XCTAssertNotNil(menu.items[3].attributedTitle)
        XCTAssertEqual(titles[4], "3 open reviews")
        XCTAssertFalse(menu.items[4].isEnabled)
        XCTAssertTrue(menu.items[5].isHidden)
        let activityHeaderIndex = try XCTUnwrap(titles.firstIndex(of: "Activity Log"))
        XCTAssertFalse(menu.items[activityHeaderIndex].isEnabled)
        XCTAssertNotNil(menu.items[activityHeaderIndex].attributedTitle)
        XCTAssertFalse(titles.contains("status: idle"))
        let lastDiscoveryIndex = try XCTUnwrap(titles.firstIndex(of: "last discovery: never"))
        XCTAssertFalse(menu.items[lastDiscoveryIndex].isEnabled)
        XCTAssertEqual(quitItem.title, "Quit")
        XCTAssertTrue(["quit:", "terminate:"].contains(NSStringFromSelector(quitItem.action!)))
        XCTAssertNil(quitItem.image)
        XCTAssertEqual(quitItem.keyEquivalent, "")
        XCTAssertEqual(quitItem.keyEquivalentModifierMask, [])
    }

    @MainActor
    func testActiveReviewsAppearAtBottomOfStatusSection() throws {
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

        XCTAssertNil(menu.items.first { $0.title == "Reviews" })
        let lastDiscoveryIndex = try XCTUnwrap(menu.items.firstIndex { $0.title == "last discovery: never" })
        XCTAssertFalse(menu.items[lastDiscoveryIndex + 1].isHidden)
        XCTAssertEqual(menu.items[lastDiscoveryIndex + 1].title, "🤖 Running review on org/repo#1")
        XCTAssertTrue(menu.items[lastDiscoveryIndex + 7].isSeparatorItem)
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

        XCTAssertTrue(menu.items[5].isHidden)
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
    func testStoppedAgentStatusDetailsOnlyShowRecentDaemonLogLines() throws {
        let appDelegate = AppDelegate()
        let lines = (1...25).map { "daemon line \($0)" }.joined(separator: "\n")
        appDelegate.setDaemonLogContentsForTesting(lines)

        appDelegate.applyStoppedAgentForTesting()

        let details = try XCTUnwrap(appDelegate.statusDetailsForTesting())
        XCTAssertTrue(details.contains("[showing last 20 log lines]"))
        XCTAssertFalse(details.contains("daemon line 5"))
        XCTAssertTrue(details.contains("daemon line 6"))
        XCTAssertTrue(details.contains("daemon line 25"))
    }

    @MainActor
    func testStoppedAgentStatusDetailsCapsVeryLargeDaemonLogText() throws {
        let appDelegate = AppDelegate()
        appDelegate.setDaemonLogContentsForTesting(String(repeating: "x", count: 20_000))

        appDelegate.applyStoppedAgentForTesting()

        let details = try XCTUnwrap(appDelegate.statusDetailsForTesting())
        XCTAssertTrue(details.contains("[truncated to last 12000 characters]"))
        XCTAssertLessThanOrEqual(details.count, 12_300)
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

    @MainActor
    func testActivityProviderLogsAreFormattedForScrollableDetails() throws {
        let appDelegate = AppDelegate()
        let activity = ActivitySnapshot(
            id: "activity-13",
            kind: "Review",
            status: "Completed",
            label: "review on temporalio/temporal#10384",
            session: ActivitySessionSnapshot(
                provider: "claude",
                providerSessionID: "provider-session",
                messages: [
                    ActivityMessageSnapshot(
                        role: "provider.stdout",
                        content: "review progress\nno findings"
                    ),
                    ActivityMessageSnapshot(
                        role: "provider.stderr",
                        content: "warning"
                    ),
                    ActivityMessageSnapshot(
                        role: "provider.sandbox",
                        content: "retry with --no-sandbox"
                    ),
                ]
            ),
            createdAtUnix: 1_000,
            updatedAtUnix: 1_017
        )

        XCTAssertEqual(
            appDelegate.activityDetailTextForTesting(activity),
            "id: activity-13\nkind: Review\nstatus: Completed"
        )
        XCTAssertEqual(
            appDelegate.activityProviderLogDetailsForTesting(activity),
            "stdout\n  review progress\n  no findings\nstderr\n  warning\nsandbox\n  retry with --no-sandbox"
        )
    }

    @MainActor
    func testReviewActivityDetailsExposeClickableReviewLink() throws {
        let appDelegate = AppDelegate()
        let activity = ActivitySnapshot(
            id: "activity-13",
            kind: "Review",
            status: "Completed",
            label: "review on temporalio/temporal#10384",
            createdAtUnix: 1_000,
            updatedAtUnix: 1_017
        )

        let link = try XCTUnwrap(appDelegate.activityReviewLinkFieldForTesting(activity))
        XCTAssertEqual(link.stringValue, "temporalio/temporal#10384")

        var effectiveRange = NSRange(location: 0, length: 0)
        let value = link.attributedStringValue.attribute(
            .link,
            at: 0,
            effectiveRange: &effectiveRange
        ) as? URL
        XCTAssertEqual(value?.absoluteString, "https://github.com/temporalio/temporal/pull/10384")
        XCTAssertEqual(effectiveRange, NSRange(location: 0, length: link.stringValue.count))
    }
}
