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

    func stop() {
        guard let process else {
            return
        }

        if process.isRunning {
            process.terminate()
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
}
