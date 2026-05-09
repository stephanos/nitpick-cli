import XCTest

@testable import NitpickAgentMacOSCore

final class MenuBarIdentityTests: XCTestCase {
    func testDefaultMenuBarIdentityUsesTemplateFriendlyImageWithSymbolFallback() {
        let identity = MenuBarIdentity()

        XCTAssertEqual(identity.imageName, "NitpickAgentMenuIcon")
        XCTAssertEqual(identity.symbolName, "text.badge.checkmark")
        XCTAssertEqual(identity.fallbackTitle, "NP")
        XCTAssertEqual(identity.accessibilityDescription, "Nitpick Agent")
    }
}
