import AppKit
import NitpickAgentMacOSCore
import Sparkle

private let agentErrorLogLineLimit = 20
private let agentErrorLogCharacterLimit = 12_000

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let identity = MenuBarIdentity()
    private let configFile = ConfigFile()
    private let loginItemManager = LoginItemManager()
    private let host = HostProcess()
    private let hostClient = HostClient()
    private var statusItem: NSStatusItem?
    private var openReviewsMenuItem: NSMenuItem?
    private var agentErrorMenuItem: NSMenuItem?
    private var lastDiscoveryRefreshMenuItem: NSMenuItem?
    private var activityHeaderMenuItem: NSMenuItem?
    private var activitySeparatorMenuItem: NSMenuItem?
    private var ongoingReviewMenuItems: [NSMenuItem] = []
    private var openAtLoginMenuItem: NSMenuItem?
    private var openAtLoginMessageItem: NSMenuItem?
    private var activityMenuItems: [NSMenuItem] = []
    private var updaterController: SPUStandardUpdaterController?
    private var refreshTimer: Timer?
    private var latestHostStatus: HostStatus?
    private var latestActivities: [ActivitySnapshot] = []
    private var latestStatusIssue: MenuStatusIssue?
    private var openAtLoginState = OpenAtLoginViewState.make(status: .notRegistered)
    private var currentStatusDetails: String?
    private var daemonLogContentsOverride: String?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)

        updaterController = SPUStandardUpdaterController(
            startingUpdater: true,
            updaterDelegate: nil,
            userDriverDelegate: nil
        )

        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        configureStatusItemButton()
        statusItem?.menu = makeMenu()

        openAtLoginState = loginItemManager.configureOnLaunch()
        installCommandLineTool()
        host.start()
        refreshMenu()
        refreshTimer = Timer.scheduledTimer(
            timeInterval: 5,
            target: self,
            selector: #selector(refreshMenu),
            userInfo: nil,
            repeats: true
        )
    }

    func applicationWillTerminate(_ notification: Notification) {
        refreshTimer?.invalidate()
        host.stop()
    }

    private func makeMenu() -> NSMenu {
        let menu = NSMenu()

        let configItem = NSMenuItem(
            title: "Open Config",
            action: #selector(openConfig),
            keyEquivalent: ""
        )
        configItem.target = self
        menu.addItem(configItem)

        let reloadConfigItem = NSMenuItem(
            title: "Reload Config",
            action: #selector(reloadConfig(_:)),
            keyEquivalent: ""
        )
        reloadConfigItem.target = self
        menu.addItem(reloadConfigItem)

        menu.addItem(NSMenuItem.separator())

        let statusHeaderMenuItem = sectionHeaderMenuItem("Status")
        menu.addItem(statusHeaderMenuItem)

        let openReviewsMenuItem = NSMenuItem(title: "no open reviews", action: nil, keyEquivalent: "")
        openReviewsMenuItem.isEnabled = false
        self.openReviewsMenuItem = openReviewsMenuItem
        menu.addItem(openReviewsMenuItem)

        let agentErrorMenuItem = NSMenuItem(title: "", action: nil, keyEquivalent: "")
        agentErrorMenuItem.isHidden = true
        agentErrorMenuItem.isEnabled = false
        self.agentErrorMenuItem = agentErrorMenuItem
        menu.addItem(agentErrorMenuItem)

        let lastDiscoveryRefreshMenuItem = NSMenuItem(title: "", action: nil, keyEquivalent: "")
        lastDiscoveryRefreshMenuItem.isEnabled = false
        self.lastDiscoveryRefreshMenuItem = lastDiscoveryRefreshMenuItem
        menu.addItem(lastDiscoveryRefreshMenuItem)

        for _ in 0 ..< 6 {
            let item = NSMenuItem(title: "", action: nil, keyEquivalent: "")
            item.isEnabled = false
            item.isHidden = true
            ongoingReviewMenuItems.append(item)
            menu.addItem(item)
        }

        menu.addItem(NSMenuItem.separator())

        let activityHeaderMenuItem = sectionHeaderMenuItem("Activity Log")
        self.activityHeaderMenuItem = activityHeaderMenuItem
        menu.addItem(activityHeaderMenuItem)

        for _ in 0 ..< 5 {
            let item = NSMenuItem(title: "", action: nil, keyEquivalent: "")
            item.isEnabled = false
            item.isHidden = true
            activityMenuItems.append(item)
            menu.addItem(item)
        }

        let activitySeparatorMenuItem = NSMenuItem.separator()
        self.activitySeparatorMenuItem = activitySeparatorMenuItem
        menu.addItem(activitySeparatorMenuItem)

        let versionItem = NSMenuItem(title: appVersionTitle(), action: nil, keyEquivalent: "")
        versionItem.isEnabled = false
        menu.addItem(versionItem)

        let updateItem = NSMenuItem(
            title: "Check for Updates...",
            action: #selector(checkForUpdates),
            keyEquivalent: ""
        )
        updateItem.target = self
        menu.addItem(updateItem)

        let openAtLoginItem = NSMenuItem(
            title: "Open at Login",
            action: #selector(toggleOpenAtLogin),
            keyEquivalent: ""
        )
        openAtLoginItem.target = self
        self.openAtLoginMenuItem = openAtLoginItem
        menu.addItem(openAtLoginItem)

        let openAtLoginMessageItem = NSMenuItem(title: "", action: nil, keyEquivalent: "")
        openAtLoginMessageItem.isEnabled = false
        openAtLoginMessageItem.isHidden = true
        self.openAtLoginMessageItem = openAtLoginMessageItem
        menu.addItem(openAtLoginMessageItem)

        let quitItem = NSMenuItem(
            title: "Quit",
            action: #selector(quit(_:)),
            keyEquivalent: ""
        )
        quitItem.target = self
        quitItem.image = nil
        quitItem.keyEquivalentModifierMask = []
        menu.addItem(quitItem)

        return menu
    }

    private func sectionHeaderMenuItem(_ title: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.isEnabled = false
        item.attributedTitle = NSAttributedString(
            string: title,
            attributes: [
                .font: NSFont.boldSystemFont(ofSize: NSFont.systemFontSize),
                .foregroundColor: NSColor.secondaryLabelColor,
            ]
        )
        return item
    }

    @objc private func refreshMenu() {
        Task { [weak self] in
            await self?.refreshHostStatus()
        }
        updateMenu()
    }

    private func refreshHostStatus() async {
        guard host.isRunning else {
            latestHostStatus = nil
            latestActivities = []
            latestStatusIssue = agentErrorStatusIssue()
            updateMenu()
            return
        }

        do {
            latestHostStatus = try await hostClient.status()
            latestActivities = (try? await hostClient.activities()) ?? []
            latestStatusIssue = nil
        } catch {
            latestHostStatus = nil
            latestActivities = []
            latestStatusIssue = agentErrorStatusIssue()
        }
        updateMenu()
    }

    private func updateMenu() {
        let snapshot = MenuSnapshot(
            hostIsRunning: host.isRunning,
            activityCount: latestHostStatus?.activityCount ?? 0,
            runningActivityCount: latestHostStatus?.runningActivityCount ?? 0,
            openReviewCount: latestHostStatus?.openReviewCount ?? 0,
            queuedReviewCount: latestHostStatus?.queuedReviewCount ?? 0,
            runningReviewCount: latestHostStatus?.runningReviewCount ?? 0,
            artifactCount: latestHostStatus?.artifactCount ?? 0,
            localOnlyArtifactCount: latestHostStatus?.localOnlyArtifactCount ?? 0,
            pendingSyncArtifactCount: latestHostStatus?.pendingSyncArtifactCount ?? 0,
            reviewSourceEnabled: latestHostStatus?.reviewSourceEnabled ?? false,
            reviewSourceLastPollUnix: latestHostStatus?.reviewSourceLastPollUnix,
            reviewSourceLastPollSummary: latestHostStatus?.reviewSourceLastPollSummary,
            statusIssue: latestStatusIssue,
            activities: latestActivities
        )
        configureAgentErrorMenuItem(snapshot)
        configureLastDiscoveryRefreshMenuItem(snapshot)
        updateOngoingReviewItems(snapshot.ongoingReviewEntries)
        updateActivityItems(snapshot.recentActivityEntries)
        updateOpenAtLoginItems()
        openReviewsMenuItem?.title = snapshot.openReviewsSummary
        statusItem?.button?.toolTip = snapshot.statusTitle
    }

    private func configureAgentErrorMenuItem(_ snapshot: MenuSnapshot) {
        currentStatusDetails = snapshot.statusDetails
        agentErrorMenuItem?.isHidden = snapshot.statusDetails == nil
        agentErrorMenuItem?.title = snapshot.statusIssue?.title ?? ""
        agentErrorMenuItem?.isEnabled = snapshot.statusDetails != nil
        agentErrorMenuItem?.target = snapshot.statusDetails == nil ? nil : self
        agentErrorMenuItem?.action = snapshot.statusDetails == nil ? nil : #selector(showStatusDetails(_:))
        agentErrorMenuItem?.image = snapshot.statusDetails == nil
            ? nil
            : NSImage(
                systemSymbolName: "exclamationmark.triangle.fill",
                accessibilityDescription: "Agent error"
            )
    }

    private func configureLastDiscoveryRefreshMenuItem(_ snapshot: MenuSnapshot) {
        guard let title = snapshot.lastDiscoveryRefreshTitle else {
            lastDiscoveryRefreshMenuItem?.isHidden = true
            lastDiscoveryRefreshMenuItem?.title = ""
            return
        }
        lastDiscoveryRefreshMenuItem?.isHidden = false
        lastDiscoveryRefreshMenuItem?.title = title
        lastDiscoveryRefreshMenuItem?.isEnabled = false
    }

    private func updateActivityItems(_ entries: [ActivityMenuEntry]) {
        let hasEntries = !entries.isEmpty
        activityHeaderMenuItem?.isHidden = !hasEntries
        activitySeparatorMenuItem?.isHidden = !hasEntries
        let font = NSFont.monospacedSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        updateMenuItems(activityMenuItems, entries: entries, font: font)
    }

    private func updateOngoingReviewItems(_ entries: [ActivityMenuEntry]) {
        updateMenuItems(
            ongoingReviewMenuItems,
            entries: entries,
            font: NSFont.systemFont(ofSize: NSFont.systemFontSize)
        )
    }

    private func updateMenuItems(_ items: [NSMenuItem], entries: [ActivityMenuEntry], font: NSFont) {
        for index in items.indices {
            let item = items[index]
            guard index < entries.count else {
                item.isHidden = true
                item.title = ""
                item.attributedTitle = nil
                item.representedObject = nil
                item.target = nil
                item.action = nil
                item.isEnabled = false
                continue
            }
            let entry = entries[index]
            item.isHidden = false
            item.title = entry.title
            item.attributedTitle = NSAttributedString(
                string: entry.title,
                attributes: [.font: font]
            )
            item.representedObject = entry.id
            item.target = entry.id == nil ? nil : self
            item.action = entry.id == nil ? nil : #selector(showActivityDetails(_:))
            item.isEnabled = entry.id != nil
        }
    }

    @objc private func checkForUpdates(_ sender: Any?) {
        updaterController?.checkForUpdates(sender)
    }

    @objc private func toggleOpenAtLogin(_ sender: NSMenuItem) {
        openAtLoginState = loginItemManager.setEnabled(sender.state != .on)
        updateOpenAtLoginItems()
    }

    @objc private func openConfig() {
        let directoryURL = configFile.url.deletingLastPathComponent()
        try? FileManager.default.createDirectory(
            at: directoryURL,
            withIntermediateDirectories: true
        )

        NSWorkspace.shared.open(configFile.url)
    }

    @objc private func reloadConfig(_ sender: Any?) {
        latestHostStatus = nil
        latestActivities = []
        host.stop()
        host.start()
        refreshMenu()
    }

    @objc private func showStatusDetails(_ sender: Any?) {
        guard let currentStatusDetails else {
            return
        }
        let alert = NSAlert()
        alert.messageText = "Agent error"
        alert.informativeText = currentStatusDetails
        alert.addButton(withTitle: "OK")
        alert.alertStyle = .warning
        alert.runModal()
    }

    @objc private func showActivityDetails(_ sender: NSMenuItem) {
        guard let activityID = sender.representedObject as? String,
              let activity = latestActivities.first(where: { $0.id == activityID })
        else {
            return
        }

        let alert = NSAlert()
        alert.messageText = activity.label ?? activity.kind
        alert.informativeText = activityDetailText(activity)
        alert.alertStyle = activity.status == "Error" ? .warning : .informational
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }

    private func activityDetailText(_ activity: ActivitySnapshot) -> String {
        var lines = [
            "id: \(activity.id)",
            "kind: \(activity.kind)",
            "status: \(activity.status)",
        ]
        if let error = activity.error, !error.isEmpty {
            lines.append("")
            lines.append(error)
        }
        return lines.joined(separator: "\n")
    }

    private func agentErrorStatusIssue() -> MenuStatusIssue {
        MenuStatusIssue(
            title: "status: agent error",
            details: agentErrorDetails()
        )
    }

    private func agentErrorDetails() -> String {
        let logURL = DaemonLogFile().url
        var lines = [
            "config: \(configFile.url.path)",
            "log: \(logURL.path)",
        ]
        let log = daemonLogContentsOverride ?? (try? String(contentsOf: logURL)) ?? ""
        let tail = boundedAgentErrorLogPreview(log)
        if !tail.isEmpty {
            lines.append("")
            lines.append(tail)
        }
        return lines.joined(separator: "\n")
    }

    private func boundedAgentErrorLogPreview(_ log: String) -> String {
        let logLines = log.split(separator: "\n", omittingEmptySubsequences: false)
        var preview = logLines
            .suffix(agentErrorLogLineLimit)
            .joined(separator: "\n")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if preview.isEmpty {
            return ""
        }
        if logLines.count > agentErrorLogLineLimit {
            preview = "[showing last \(agentErrorLogLineLimit) log lines]\n\(preview)"
        }
        if preview.count > agentErrorLogCharacterLimit {
            preview = "[truncated to last \(agentErrorLogCharacterLimit) characters]\n"
                + String(preview.suffix(agentErrorLogCharacterLimit))
        }
        return preview
    }

    @objc private func quit(_ sender: Any?) {
        NSApp.terminate(sender)
    }

    private func updateOpenAtLoginItems() {
        switch openAtLoginState.status {
        case .enabled:
            openAtLoginMenuItem?.state = .on
        case .requiresApproval:
            openAtLoginMenuItem?.state = .mixed
        case .notRegistered, .notFound, .unknown:
            openAtLoginMenuItem?.state = .off
        }

        if let message = openAtLoginState.message, !message.isEmpty {
            openAtLoginMessageItem?.title = message
            openAtLoginMessageItem?.isHidden = false
        } else {
            openAtLoginMessageItem?.title = ""
            openAtLoginMessageItem?.isHidden = true
        }
    }

    private func appVersionTitle() -> String {
        if let shortVersion = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString")
            as? String, !shortVersion.isEmpty
        {
            return "Nitpick Agent v\(shortVersion)"
        }

        if let bundleVersion = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion")
            as? String, !bundleVersion.isEmpty
        {
            return "Nitpick Agent v\(bundleVersion)"
        }

        return "Nitpick Agent"
    }

    private func configureStatusItemButton() {
        guard let button = statusItem?.button else {
            return
        }

        if let image = menuBarImage() {
            image.isTemplate = true
            image.size = NSSize(width: 18, height: 18)
            button.image = image
            button.title = ""
        } else if let image = NSImage(
            systemSymbolName: identity.symbolName,
            accessibilityDescription: identity.accessibilityDescription
        ) {
            image.isTemplate = true
            button.image = image
            button.title = ""
        } else {
            button.title = identity.fallbackTitle
        }
    }

    private func menuBarImage() -> NSImage? {
        guard let url = Bundle.main.url(forResource: identity.imageName, withExtension: "svg") else {
            return nil
        }

        return NSImage(contentsOf: url)
    }

    private func installCommandLineTool() {
        guard let executableURL = bundledExecutableURL(named: "nitpick") else {
            return
        }

        let installer = CommandLineInstaller(bundledExecutableURL: executableURL)
        _ = try? installer.install()
    }

    private func bundledExecutableURL(named name: String) -> URL? {
        if let bundled = Bundle.main.url(forAuxiliaryExecutable: name) {
            return bundled
        }

        return Bundle.main.executableURL?
            .deletingLastPathComponent()
            .appendingPathComponent(name)
    }
}

#if DEBUG
extension AppDelegate {
    func makeMenuForTesting() -> NSMenu {
        makeMenu()
    }

    func applyMenuSnapshotForTesting(_ snapshot: MenuSnapshot) {
        configureAgentErrorMenuItem(snapshot)
        configureLastDiscoveryRefreshMenuItem(snapshot)
        updateOngoingReviewItems(snapshot.ongoingReviewEntries)
        updateActivityItems(snapshot.recentActivityEntries)
        openReviewsMenuItem?.title = snapshot.openReviewsSummary
    }

    func setStatusForTesting(hostStatus: HostStatus?) {
        latestHostStatus = hostStatus
        latestActivities = []
        latestStatusIssue = nil
        updateMenu()
    }

    func setActivitiesForTesting(_ activities: [ActivitySnapshot]) {
        latestHostStatus = nil
        latestActivities = activities
        latestStatusIssue = nil
        updateMenu()
    }

    func setDaemonLogContentsForTesting(_ contents: String) {
        daemonLogContentsOverride = contents
    }

    func applyStoppedAgentForTesting() {
        latestHostStatus = nil
        latestActivities = []
        latestStatusIssue = agentErrorStatusIssue()
        updateMenu()
    }

    func statusDetailsForTesting() -> String? {
        currentStatusDetails
    }
}
#endif
