import XCTest

@testable import NitpickAgentMacOSCore

final class CommandLineInstallerTests: XCTestCase {
    func testInstallsBundledCliAsSymlink() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString)
        let bundledDirectory = root.appendingPathComponent("bundle")
        let installDirectory = root.appendingPathComponent("bin")
        let bundledCli = bundledDirectory.appendingPathComponent("nitpick-agent")

        try FileManager.default.createDirectory(
            at: bundledDirectory,
            withIntermediateDirectories: true
        )
        FileManager.default.createFile(
            atPath: bundledCli.path,
            contents: Data("#!/bin/sh\n".utf8)
        )
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o755],
            ofItemAtPath: bundledCli.path
        )

        let installer = CommandLineInstaller(
            bundledExecutableURL: bundledCli,
            installDirectoryURL: installDirectory
        )

        let installedURL = try installer.install()

        var isDirectory: ObjCBool = false
        XCTAssertTrue(FileManager.default.fileExists(atPath: installedURL.path, isDirectory: &isDirectory))
        XCTAssertFalse(isDirectory.boolValue)
        XCTAssertEqual(
            try FileManager.default.destinationOfSymbolicLink(atPath: installedURL.path),
            bundledCli.path
        )

        try FileManager.default.removeItem(at: root)
    }
}
