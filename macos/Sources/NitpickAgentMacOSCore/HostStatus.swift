public struct HostStatus: Decodable, Equatable {
    public var activityCount: Int
    public var runningActivityCount: Int
    public var completedActivityCount: Int
    public var errorActivityCount: Int
    public var artifactCount: Int
    public var localOnlyArtifactCount: Int
    public var pendingSyncArtifactCount: Int
    public var provider: String
    public var model: String?

    private enum CodingKeys: String, CodingKey {
        case activityCount = "activity_count"
        case runningActivityCount = "running_activity_count"
        case completedActivityCount = "completed_activity_count"
        case errorActivityCount = "error_activity_count"
        case artifactCount = "artifact_count"
        case localOnlyArtifactCount = "local_only_artifact_count"
        case pendingSyncArtifactCount = "pending_sync_artifact_count"
        case provider
        case model
    }
}
