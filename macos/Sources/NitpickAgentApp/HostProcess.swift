import Darwin
import Foundation

final class HostProcess {
    private var process: Process?

    var isRunning: Bool {
        process?.isRunning == true
    }

    func start() {
        if isRunning {
            return
        }

        guard let executableURL = hostExecutableURL() else {
            return
        }

        let process = Process()
        process.executableURL = executableURL
        process.arguments = ["daemon"]
        process.standardOutput = Pipe()
        process.standardError = Pipe()

        do {
            try process.run()
            self.process = process
        } catch {
            self.process = nil
        }
    }

    func stop(timeout: TimeInterval = 5) {
        guard let process else {
            return
        }

        if process.isRunning {
            process.terminate()
            waitForExit(process, timeout: timeout)
        }

        if process.isRunning {
            kill(process.processIdentifier, SIGKILL)
            waitForExit(process, timeout: 1)
        }
        self.process = nil
    }

    private func hostExecutableURL() -> URL? {
        if let bundled = Bundle.main.url(forAuxiliaryExecutable: "nitpick-agent-host") {
            return bundled
        }

        return Bundle.main.executableURL?
            .deletingLastPathComponent()
            .appendingPathComponent("nitpick-agent-host")
    }

    private func waitForExit(_ process: Process, timeout: TimeInterval) {
        let deadline = Date().addingTimeInterval(timeout)
        while process.isRunning && Date() < deadline {
            Thread.sleep(forTimeInterval: 0.05)
        }
    }
}
