import AppKit
import NitpickAgentMacOSCore
import Sparkle

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let identity = MenuBarIdentity()
    private let configFile = ConfigFile()
    private let loginItemManager = LoginItemManager()
    private let host = HostProcess()
    private let hostClient = HostClient()
    private var statusItem: NSStatusItem?
    private var statusMenuItem: NSMenuItem?
    private var openAtLoginMenuItem: NSMenuItem?
    private var openAtLoginMessageItem: NSMenuItem?
    private var activityMenuItems: [NSMenuItem] = []
    private var updaterController: SPUStandardUpdaterController?
    private var refreshTimer: Timer?
    private var latestHostStatus: HostStatus?
    private var latestActivities: [ActivitySnapshot] = []
    private var openAtLoginState = OpenAtLoginViewState.make(status: .notRegistered)
    private var currentStatusDetails: String?

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

        let statusMenuItem = NSMenuItem(title: "Status: Starting", action: nil, keyEquivalent: "")
        statusMenuItem.isEnabled = false
        self.statusMenuItem = statusMenuItem
        menu.addItem(statusMenuItem)

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

        for _ in 0 ..< 5 {
            let item = NSMenuItem(title: "", action: nil, keyEquivalent: "")
            item.isEnabled = false
            item.isHidden = true
            activityMenuItems.append(item)
            menu.addItem(item)
        }

        menu.addItem(NSMenuItem.separator())

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
            updateMenu()
            return
        }

        latestHostStatus = try? await hostClient.status()
        latestActivities = (try? await hostClient.activities()) ?? []
        updateMenu()
    }

    private func updateMenu() {
        let snapshot = MenuSnapshot(
            hostIsRunning: host.isRunning,
            activityCount: latestHostStatus?.activityCount ?? 0,
            runningActivityCount: latestHostStatus?.runningActivityCount ?? 0,
            artifactCount: latestHostStatus?.artifactCount ?? 0,
            localOnlyArtifactCount: latestHostStatus?.localOnlyArtifactCount ?? 0,
            pendingSyncArtifactCount: latestHostStatus?.pendingSyncArtifactCount ?? 0,
            reviewSourceEnabled: latestHostStatus?.reviewSourceEnabled ?? false,
            reviewSourceLastPollSummary: latestHostStatus?.reviewSourceLastPollSummary,
            activities: latestActivities
        )
        configureStatusMenuItem(snapshot)
        updateActivityItems(snapshot.recentActivityTitles)
        updateOpenAtLoginItems()
        statusItem?.button?.toolTip = snapshot.statusTitle
    }

    private func configureStatusMenuItem(_ snapshot: MenuSnapshot) {
        currentStatusDetails = snapshot.statusDetails
        statusMenuItem?.title = snapshot.statusTitle
        statusMenuItem?.isEnabled = snapshot.statusDetails != nil
        statusMenuItem?.target = snapshot.statusDetails == nil ? nil : self
        statusMenuItem?.action = snapshot.statusDetails == nil ? nil : #selector(showStatusDetails(_:))
        statusMenuItem?.image = snapshot.statusDetails == nil
            ? nil
            : NSImage(
                systemSymbolName: "exclamationmark.triangle.fill",
                accessibilityDescription: "Discovery error"
            )
    }

    private func updateActivityItems(_ titles: [String]) {
        let font = NSFont.monospacedSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
        for index in activityMenuItems.indices {
            let item = activityMenuItems[index]
            guard index < titles.count else {
                item.isHidden = true
                item.title = ""
                item.attributedTitle = nil
                continue
            }
            item.isHidden = false
            item.attributedTitle = NSAttributedString(
                string: titles[index],
                attributes: [.font: font]
            )
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

        if !FileManager.default.fileExists(atPath: configFile.url.path) {
            FileManager.default.createFile(atPath: configFile.url.path, contents: nil)
        }

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
        alert.messageText = "Discovery error"
        alert.informativeText = currentStatusDetails
        alert.addButton(withTitle: "OK")
        alert.alertStyle = .warning
        alert.runModal()
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
        configureStatusMenuItem(snapshot)
    }

    func setStatusForTesting(hostStatus: HostStatus?) {
        latestHostStatus = hostStatus
        latestActivities = []
        updateMenu()
    }

    func statusDetailsForTesting() -> String? {
        currentStatusDetails
    }
}
#endif
