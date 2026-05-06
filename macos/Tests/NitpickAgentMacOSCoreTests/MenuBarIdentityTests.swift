import XCTest

@testable import NitpickAgentMacOSCore

final class MenuBarIdentityTests: XCTestCase {
    func testDefaultMenuBarIdentityUsesTemplateFriendlySymbol() {
        let identity = MenuBarIdentity()

        XCTAssertEqual(identity.symbolName, "text.badge.checkmark")
        XCTAssertEqual(identity.fallbackTitle, "NP")
        XCTAssertEqual(identity.accessibilityDescription, "Nitpick Agent")
    }
}
