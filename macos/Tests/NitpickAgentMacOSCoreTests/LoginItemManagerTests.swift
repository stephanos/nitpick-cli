import XCTest

@testable import NitpickAgentMacOSCore

private final class FakeLoginItemService: LoginItemService {
    var status: OpenAtLoginStatus
    var didRegister = false
    var registerError: Error?

    init(status: OpenAtLoginStatus) {
        self.status = status
    }

    func register() throws {
        didRegister = true
        if let registerError {
            throw registerError
        }
        status = .enabled
    }

    func unregister() throws {
        status = .notRegistered
    }
}

private struct FakeError: Error, LocalizedError {
    let errorDescription: String? = "fake failure"
}

final class LoginItemManagerTests: XCTestCase {
    func testConfigureOnLaunchDefaultsToEnabled() {
        let defaultsName = "NitpickAgentTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: defaultsName)!
        defaults.removePersistentDomain(forName: defaultsName)

        let service = FakeLoginItemService(status: .notRegistered)
        let manager = LoginItemManager(
            defaultsKey: "openAtLoginEnabled",
            defaults: defaults,
            service: service
        )

        let state = manager.configureOnLaunch()

        XCTAssertTrue(defaults.bool(forKey: "openAtLoginEnabled"))
        XCTAssertTrue(service.didRegister)
        XCTAssertEqual(state.status, .enabled)
    }

    func testConfigureOnLaunchSuppressesTransientNotFound() {
        let defaultsName = "NitpickAgentTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: defaultsName)!
        defaults.removePersistentDomain(forName: defaultsName)

        let service = FakeLoginItemService(status: .notFound)
        let manager = LoginItemManager(
            defaultsKey: "openAtLoginEnabled",
            defaults: defaults,
            service: service
        )

        let state = manager.configureOnLaunch()

        XCTAssertTrue(defaults.bool(forKey: "openAtLoginEnabled"))
        XCTAssertFalse(service.didRegister)
        XCTAssertEqual(state.status, .notRegistered)
        XCTAssertNil(state.message)
    }

    func testSetEnabledAttemptsRegistrationFromNotFound() {
        let defaultsName = "NitpickAgentTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: defaultsName)!
        defaults.removePersistentDomain(forName: defaultsName)

        let service = FakeLoginItemService(status: .notFound)
        let manager = LoginItemManager(
            defaultsKey: "openAtLoginEnabled",
            defaults: defaults,
            service: service
        )

        let state = manager.setEnabled(true)

        XCTAssertTrue(defaults.bool(forKey: "openAtLoginEnabled"))
        XCTAssertTrue(service.didRegister)
        XCTAssertEqual(state.status, .enabled)
    }

    func testSetEnabledSurfacesRegistrationErrors() {
        let defaultsName = "NitpickAgentTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: defaultsName)!
        defaults.removePersistentDomain(forName: defaultsName)

        let service = FakeLoginItemService(status: .notRegistered)
        service.registerError = FakeError()
        let manager = LoginItemManager(
            defaultsKey: "openAtLoginEnabled",
            defaults: defaults,
            service: service
        )

        let state = manager.setEnabled(true)

        XCTAssertEqual(state.message, "Open at login unavailable: fake failure")
    }
}
