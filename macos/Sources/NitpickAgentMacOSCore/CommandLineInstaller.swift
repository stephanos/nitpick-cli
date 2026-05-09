import Foundation

public struct CommandLineInstaller {
    public let bundledExecutableURL: URL
    public let installDirectoryURL: URL
    public let executableName: String

    public init(
        bundledExecutableURL: URL,
        installDirectoryURL: URL = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".local")
            .appendingPathComponent("bin"),
        executableName: String = "nitpick"
    ) {
        self.bundledExecutableURL = bundledExecutableURL
        self.installDirectoryURL = installDirectoryURL
        self.executableName = executableName
    }

    public func install() throws -> URL {
        try FileManager.default.createDirectory(
            at: installDirectoryURL,
            withIntermediateDirectories: true
        )

        let installedURL = installDirectoryURL.appendingPathComponent(executableName)
        if FileManager.default.fileExists(atPath: installedURL.path) {
            try FileManager.default.removeItem(at: installedURL)
        }

        try FileManager.default.createSymbolicLink(
            at: installedURL,
            withDestinationURL: bundledExecutableURL
        )
        return installedURL
    }
}
