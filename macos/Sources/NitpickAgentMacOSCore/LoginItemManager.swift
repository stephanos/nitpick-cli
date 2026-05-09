import Foundation
import ServiceManagement

public enum OpenAtLoginStatus: Equatable {
    case enabled
    case requiresApproval
    case notRegistered
    case notFound
    case unknown
}

public struct OpenAtLoginViewState: Equatable {
    public let status: OpenAtLoginStatus
    public let message: String?

    public static func make(status: OpenAtLoginStatus, errorMessage: String? = nil)
        -> OpenAtLoginViewState
    {
        if let errorMessage, !errorMessage.isEmpty {
            return OpenAtLoginViewState(
                status: status,
                message: "Open at login unavailable: \(errorMessage)"
            )
        }

        switch status {
        case .enabled, .notRegistered:
            return OpenAtLoginViewState(status: status, message: nil)
        case .requiresApproval:
            return OpenAtLoginViewState(
                status: status,
                message: "Open at login pending approval in System Settings"
            )
        case .notFound:
            return OpenAtLoginViewState(
                status: status,
                message: "Open at login unavailable for this app build"
            )
        case .unknown:
            return OpenAtLoginViewState(status: status, message: "Open at login status unknown")
        }
    }
}

public protocol LoginItemService {
    var status: OpenAtLoginStatus { get }
    func register() throws
    func unregister() throws
}

public struct MainAppLoginItemService: LoginItemService {
    public var status: OpenAtLoginStatus {
        switch SMAppService.mainApp.status {
        case .enabled:
            return .enabled
        case .requiresApproval:
            return .requiresApproval
        case .notRegistered:
            return .notRegistered
        case .notFound:
            return .notFound
        @unknown default:
            return .unknown
        }
    }

    public init() {}

    public func register() throws {
        try SMAppService.mainApp.register()
    }

    public func unregister() throws {
        try SMAppService.mainApp.unregister()
    }
}

public final class LoginItemManager {
    private let defaultsKey: String
    private let defaults: UserDefaults
    private let service: any LoginItemService

    public init(
        defaultsKey: String = "openAtLoginEnabled",
        defaults: UserDefaults = .standard,
        service: any LoginItemService = MainAppLoginItemService()
    ) {
        self.defaultsKey = defaultsKey
        self.defaults = defaults
        self.service = service
    }

    public func configureOnLaunch() -> OpenAtLoginViewState {
        let hasStoredPreference = defaults.object(forKey: defaultsKey) != nil
        let shouldOpenAtLogin = hasStoredPreference ? defaults.bool(forKey: defaultsKey) : true
        let currentStatus = service.status

        if !hasStoredPreference {
            defaults.set(shouldOpenAtLogin, forKey: defaultsKey)
        }

        if currentStatus == .notFound {
            return OpenAtLoginViewState.make(status: .notRegistered)
        }

        return setEnabled(shouldOpenAtLogin, persist: false)
    }

    public func setEnabled(_ shouldEnable: Bool, persist: Bool = true) -> OpenAtLoginViewState {
        if persist {
            defaults.set(shouldEnable, forKey: defaultsKey)
        }

        do {
            let currentStatus = service.status

            switch (shouldEnable, currentStatus) {
            case (true, .enabled), (false, .notRegistered):
                break
            case (true, _):
                try service.register()
            case (false, _):
                try service.unregister()
            }

            return OpenAtLoginViewState.make(status: service.status)
        } catch {
            return OpenAtLoginViewState.make(
                status: service.status,
                errorMessage: error.localizedDescription
            )
        }
    }
}
