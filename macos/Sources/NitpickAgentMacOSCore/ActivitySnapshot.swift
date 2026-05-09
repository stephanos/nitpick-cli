import Foundation

public struct ActivitySnapshot: Decodable, Equatable {
    public var id: String
    public var kind: String
    public var status: String
    public var label: String?
    public var createdAtUnix: UInt64
    public var updatedAtUnix: UInt64

    private enum CodingKeys: String, CodingKey {
        case id
        case kind
        case status
        case label
        case createdAtUnix = "created_at_unix"
        case updatedAtUnix = "updated_at_unix"
    }
}
