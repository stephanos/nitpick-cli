import Foundation

public struct ActivitySnapshot: Decodable, Equatable {
    public var id: String
    public var kind: String
    public var status: String
    public var label: String?
    public var error: String?
    public var session: ActivitySessionSnapshot?
    public var createdAtUnix: UInt64
    public var updatedAtUnix: UInt64

    public init(
        id: String,
        kind: String,
        status: String,
        label: String?,
        error: String? = nil,
        session: ActivitySessionSnapshot? = nil,
        createdAtUnix: UInt64,
        updatedAtUnix: UInt64
    ) {
        self.id = id
        self.kind = kind
        self.status = status
        self.label = label
        self.error = error
        self.session = session
        self.createdAtUnix = createdAtUnix
        self.updatedAtUnix = updatedAtUnix
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case kind
        case status
        case label
        case error
        case session
        case createdAtUnix = "created_at_unix"
        case updatedAtUnix = "updated_at_unix"
    }
}

public struct ActivitySessionSnapshot: Decodable, Equatable {
    public var provider: String?
    public var providerSessionID: String?
    public var messages: [ActivityMessageSnapshot]

    public init(
        provider: String? = nil,
        providerSessionID: String? = nil,
        messages: [ActivityMessageSnapshot] = []
    ) {
        self.provider = provider
        self.providerSessionID = providerSessionID
        self.messages = messages
    }

    private enum CodingKeys: String, CodingKey {
        case provider
        case providerSessionID = "provider_session_id"
        case messages
    }
}

public struct ActivityMessageSnapshot: Decodable, Equatable {
    public var role: String
    public var content: String

    public init(role: String, content: String) {
        self.role = role
        self.content = content
    }
}

public struct ActivityMenuEntry: Equatable {
    public let id: String?
    public let title: String
    public let isEnabled: Bool
    public let isHidden: Bool

    public init(id: String?, title: String, isEnabled: Bool? = nil, isHidden: Bool = false) {
        self.id = id
        self.title = title
        self.isEnabled = isEnabled ?? (id != nil)
        self.isHidden = isHidden
    }
}

public struct MenuStatusIssue: Equatable {
    public var title: String
    public var details: String

    public init(title: String, details: String) {
        self.title = title
        self.details = details
    }
}
