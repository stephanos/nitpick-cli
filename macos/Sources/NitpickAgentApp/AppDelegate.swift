import AppKit
import NitpickAgentMacOSCore
import Sparkle

private let agentErrorLogLineLimit = 20
private let agentErrorLogCharacterLimit = 12_000
private let agentErrorDetailsViewSize = NSSize(width: 640, height: 320)

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
        let presentation = MenuPresentation(snapshot: snapshot)
        configureAgentErrorMenuItem(
            presentation.status.agentErrorItem,
            statusDetails: presentation.status.details
        )
        configureLastDiscoveryRefreshMenuItem(presentation.lastDiscoveryRefresh)
        updateOngoingReviewItems(presentation.ongoingReviews)
        updateActivityItems(presentation.recentActivities)
        updateOpenAtLoginItems()
        openReviewsMenuItem?.title = presentation.status.openReviewsTitle
        statusItem?.button?.toolTip = presentation.status.title
    }

    private func configureAgentErrorMenuItem(_ item: ActivityMenuEntry, statusDetails: String?) {
        currentStatusDetails = statusDetails
        agentErrorMenuItem?.isHidden = item.isHidden
        agentErrorMenuItem?.title = item.title
        agentErrorMenuItem?.isEnabled = item.isEnabled
        agentErrorMenuItem?.target = item.isEnabled ? self : nil
        agentErrorMenuItem?.action = item.isEnabled ? #selector(showStatusDetails(_:)) : nil
        agentErrorMenuItem?.image = item.isHidden
            ? nil
            : NSImage(
                systemSymbolName: "exclamationmark.triangle.fill",
                accessibilityDescription: "Agent error"
            )
    }

    private func configureLastDiscoveryRefreshMenuItem(_ item: ActivityMenuEntry) {
        lastDiscoveryRefreshMenuItem?.isHidden = item.isHidden
        lastDiscoveryRefreshMenuItem?.title = item.title
        lastDiscoveryRefreshMenuItem?.isEnabled = item.isEnabled
    }

    private func updateActivityItems(_ section: MenuActivitySectionPresentation) {
        activityHeaderMenuItem?.isHidden = section.isHidden
        activitySeparatorMenuItem?.isHidden = section.isHidden
        let font = NSFont.monospacedSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        updateMenuItems(activityMenuItems, entries: section.items, font: font)
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
            item.isHidden = entry.isHidden
            item.title = entry.title
            item.attributedTitle = NSAttributedString(
                string: entry.title,
                attributes: [.font: font]
            )
            item.representedObject = entry.id
            item.target = entry.isEnabled ? self : nil
            item.action = entry.isEnabled ? #selector(showActivityDetails(_:)) : nil
            item.isEnabled = entry.isEnabled
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
        alert.informativeText = "Review the details below."
        alert.accessoryView = makeScrollableDetailsView(currentStatusDetails)
        alert.addButton(withTitle: "OK")
        alert.alertStyle = .warning
        alert.runModal()
    }

    private func makeScrollableDetailsView(_ details: String) -> NSScrollView {
        let textView = NSTextView(frame: NSRect(origin: .zero, size: agentErrorDetailsViewSize))
        textView.string = details
        textView.isEditable = false
        textView.isSelectable = true
        textView.font = NSFont.monospacedSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        textView.textContainerInset = NSSize(width: 8, height: 8)
        textView.autoresizingMask = [.width]
        textView.textContainer?.containerSize = NSSize(
            width: agentErrorDetailsViewSize.width,
            height: .greatestFiniteMagnitude
        )
        textView.textContainer?.widthTracksTextView = true

        let scrollView = NSScrollView(frame: NSRect(origin: .zero, size: agentErrorDetailsViewSize))
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = false
        scrollView.autohidesScrollers = true
        scrollView.borderType = .bezelBorder
        scrollView.documentView = textView
        return scrollView
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
        if let accessoryView = activityDetailsAccessoryView(activity) {
            alert.accessoryView = accessoryView
        }
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

    private func activityDetailsAccessoryView(_ activity: ActivitySnapshot) -> NSView? {
        let reviewLinkField = activityReviewLinkField(activity)
        let providerLogView = activityProviderLogDetails(activity).map(makeScrollableDetailsView)
        guard reviewLinkField != nil || providerLogView != nil else {
            return nil
        }
        guard let reviewLinkField, let providerLogView else {
            return reviewLinkField ?? providerLogView
        }

        let stackView = NSStackView(views: [reviewLinkField, providerLogView])
        stackView.orientation = .vertical
        stackView.alignment = .leading
        stackView.spacing = 8
        stackView.setFrameSize(NSSize(
            width: agentErrorDetailsViewSize.width,
            height: reviewLinkField.intrinsicContentSize.height + stackView.spacing + agentErrorDetailsViewSize.height
        ))
        return stackView
    }

    private func activityReviewLinkField(_ activity: ActivitySnapshot) -> NSTextField? {
        guard let reviewLink = activityReviewLink(activity) else {
            return nil
        }
        let field = NSTextField(wrappingLabelWithString: reviewLink.display)
        field.isSelectable = true
        field.allowsEditingTextAttributes = true
        field.attributedStringValue = NSAttributedString(
            string: reviewLink.display,
            attributes: [
                .link: reviewLink.url,
                .foregroundColor: NSColor.linkColor,
                .underlineStyle: NSUnderlineStyle.single.rawValue,
            ]
        )
        field.toolTip = reviewLink.url.absoluteString
        field.setFrameSize(NSSize(
            width: agentErrorDetailsViewSize.width,
            height: field.intrinsicContentSize.height
        ))
        return field
    }

    private func activityReviewLink(_ activity: ActivitySnapshot) -> ActivityReviewLink? {
        guard activity.kind == "Review", let label = activity.label else {
            return nil
        }
        let pattern = #"([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)#([0-9]+)"#
        guard let expression = try? NSRegularExpression(pattern: pattern),
              let match = expression.firstMatch(
                in: label,
                range: NSRange(label.startIndex..<label.endIndex, in: label)
              ),
              let repositoryRange = Range(match.range(at: 1), in: label),
              let numberRange = Range(match.range(at: 2), in: label)
        else {
            return nil
        }

        let repository = String(label[repositoryRange])
        let number = String(label[numberRange])
        return ActivityReviewLink(
            display: "\(repository)#\(number)",
            url: URL(string: "https://github.com/\(repository)/pull/\(number)")!
        )
    }

    private func activityProviderLogDetails(_ activity: ActivitySnapshot) -> String? {
        let logs: [String] = activity.session?.messages
            .filter { message in
                message.role == "provider.stdout"
                    || message.role == "provider.stderr"
                    || message.role == "provider.sandbox"
            }
            .map { message in
                let label = message.role.replacingOccurrences(of: "provider.", with: "")
                return "\(label)\n\(indentLogBlock(message.content))"
            } ?? []
        guard !logs.isEmpty else {
            return nil
        }
        return logs.joined(separator: "\n")
    }

    private func indentLogBlock(_ value: String) -> String {
        value
            .split(separator: "\n", omittingEmptySubsequences: false)
            .map { "  \($0)" }
            .joined(separator: "\n")
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

private struct ActivityReviewLink {
    var display: String
    var url: URL
}

#if DEBUG
extension AppDelegate {
    func makeMenuForTesting() -> NSMenu {
        makeMenu()
    }

    func applyMenuSnapshotForTesting(_ snapshot: MenuSnapshot) {
        let presentation = MenuPresentation(snapshot: snapshot)
        configureAgentErrorMenuItem(
            presentation.status.agentErrorItem,
            statusDetails: presentation.status.details
        )
        configureLastDiscoveryRefreshMenuItem(presentation.lastDiscoveryRefresh)
        updateOngoingReviewItems(presentation.ongoingReviews)
        updateActivityItems(presentation.recentActivities)
        openReviewsMenuItem?.title = presentation.status.openReviewsTitle
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

    func activityDetailTextForTesting(_ activity: ActivitySnapshot) -> String {
        activityDetailText(activity)
    }

    func activityProviderLogDetailsForTesting(_ activity: ActivitySnapshot) -> String? {
        activityProviderLogDetails(activity)
    }

    func activityReviewLinkFieldForTesting(_ activity: ActivitySnapshot) -> NSTextField? {
        activityReviewLinkField(activity)
    }
}
#endif
