import Darwin
import Foundation
import NitpickAgentMacOSCore

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
        process.environment = HostProcessEnvironment.values()
        let logURL = DaemonLogFile().url
        do {
            try FileManager.default.createDirectory(
                at: logURL.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            if !FileManager.default.fileExists(atPath: logURL.path) {
                FileManager.default.createFile(atPath: logURL.path, contents: nil)
            }
            let logHandle = try FileHandle(forWritingTo: logURL)
            try logHandle.seekToEnd()
            process.standardOutput = logHandle
            process.standardError = logHandle
        } catch {
            process.standardOutput = Pipe()
            process.standardError = Pipe()
        }

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

enum HostProcessEnvironment {
    private static let fallbackPathEntries = [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ]

    static func values(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        homeDirectoryURL: URL = FileManager.default.homeDirectoryForCurrentUser
    ) -> [String: String] {
        var values = environment
        values["PATH"] = commandLookupPath(
            environment: environment,
            homeDirectoryURL: homeDirectoryURL
        )
        return values
    }

    static func commandLookupPath(
        environment: [String: String],
        homeDirectoryURL: URL
    ) -> String {
        var paths: [String] = []
        appendPathEntries(environment["PATH"], to: &paths)
        paths.append(homeDirectoryURL.appendingPathComponent(".local/bin").path)
        paths.append(homeDirectoryURL.appendingPathComponent("bin").path)
        paths.append(contentsOf: fallbackPathEntries)

        var seen = Set<String>()
        return paths.filter { path in
            guard !path.isEmpty, !seen.contains(path) else {
                return false
            }
            seen.insert(path)
            return true
        }.joined(separator: ":")
    }

    private static func appendPathEntries(_ path: String?, to paths: inout [String]) {
        guard let path else {
            return
        }
        paths.append(contentsOf: path.split(separator: ":").map(String.init))
    }
}
