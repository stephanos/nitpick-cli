import AppKit
import NitpickAgentMacOSCore
import Sparkle

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let identity = MenuBarIdentity()
    private let host = HostProcess()
    private let hostClient = HostClient()
    private var statusItem: NSStatusItem?
    private var statusMenuItem: NSMenuItem?
    private var updaterController: SPUStandardUpdaterController?
    private var refreshTimer: Timer?
    private var latestHostStatus: HostStatus?

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

        menu.addItem(NSMenuItem.separator())

        let restartItem = NSMenuItem(
            title: "Restart Host",
            action: #selector(restartHost),
            keyEquivalent: ""
        )
        restartItem.target = self
        menu.addItem(restartItem)

        let updateItem = NSMenuItem(
            title: "Check for Updates...",
            action: #selector(checkForUpdates),
            keyEquivalent: ""
        )
        updateItem.target = self
        menu.addItem(updateItem)

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
            updateMenu()
            return
        }

        latestHostStatus = try? await hostClient.status()
        updateMenu()
    }

    private func updateMenu() {
        let snapshot = MenuSnapshot(
            hostIsRunning: host.isRunning,
            activityCount: latestHostStatus?.activityCount ?? 0,
            runningActivityCount: latestHostStatus?.runningActivityCount ?? 0,
            artifactCount: latestHostStatus?.artifactCount ?? 0,
            localOnlyArtifactCount: latestHostStatus?.localOnlyArtifactCount ?? 0,
            pendingSyncArtifactCount: latestHostStatus?.pendingSyncArtifactCount ?? 0
        )
        statusMenuItem?.title = snapshot.statusTitle
        statusItem?.button?.toolTip = snapshot.statusTitle
    }

    @objc private func restartHost() {
        host.stop()
        host.start()
        refreshMenu()
    }

    @objc private func checkForUpdates(_ sender: Any?) {
        updaterController?.checkForUpdates(sender)
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
