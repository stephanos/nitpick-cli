import AppKit
import NitpickAgentMacOSCore
import Sparkle

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let identity = MenuBarIdentity()
    private let configFile = ConfigFile()
    private let host = HostProcess()
    private let hostClient = HostClient()
    private var statusItem: NSStatusItem?
    private var statusMenuItem: NSMenuItem?
    private var githubMenuItem: NSMenuItem?
    private var activityMenuItems: [NSMenuItem] = []
    private var updaterController: SPUStandardUpdaterController?
    private var refreshTimer: Timer?
    private var latestHostStatus: HostStatus?
    private var latestActivities: [ActivitySnapshot] = []

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

        let githubMenuItem = NSMenuItem(title: "GitHub: Starting", action: nil, keyEquivalent: "")
        githubMenuItem.isEnabled = false
        self.githubMenuItem = githubMenuItem
        menu.addItem(githubMenuItem)

        menu.addItem(NSMenuItem.separator())

        let restartItem = NSMenuItem(
            title: "Restart Host",
            action: #selector(restartHost),
            keyEquivalent: ""
        )
        restartItem.target = self
        menu.addItem(restartItem)

        let configItem = NSMenuItem(
            title: "Open Config",
            action: #selector(openConfig),
            keyEquivalent: ","
        )
        configItem.target = self
        menu.addItem(configItem)

        let updateItem = NSMenuItem(
            title: "Check for Updates...",
            action: #selector(checkForUpdates),
            keyEquivalent: ""
        )
        updateItem.target = self
        menu.addItem(updateItem)

        menu.addItem(NSMenuItem.separator())

        for _ in 0 ..< 5 {
            let item = NSMenuItem(title: "", action: nil, keyEquivalent: "")
            item.isEnabled = false
            item.isHidden = true
            activityMenuItems.append(item)
            menu.addItem(item)
        }

        menu.addItem(NSMenuItem.separator())

        let quitItem = NSMenuItem(
            title: "Quit",
            action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q"
        )
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
            githubDiscoveryEnabled: latestHostStatus?.githubDiscoveryEnabled ?? false,
            githubLastPollSummary: latestHostStatus?.githubLastPollSummary,
            activities: latestActivities
        )
        statusMenuItem?.title = snapshot.statusTitle
        githubMenuItem?.title = snapshot.githubTitle
        updateActivityItems(snapshot.recentActivityTitles)
        statusItem?.button?.toolTip = snapshot.statusTitle
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

    @objc private func restartHost() {
        host.stop()
        host.start()
        refreshMenu()
    }

    @objc private func checkForUpdates(_ sender: Any?) {
        updaterController?.checkForUpdates(sender)
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

    private func configureStatusItemButton() {
        guard let button = statusItem?.button else {
            return
        }

        if let image = NSImage(
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

    private func installCommandLineTool() {
        guard let executableURL = bundledExecutableURL(named: "nitpick-agent") else {
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
