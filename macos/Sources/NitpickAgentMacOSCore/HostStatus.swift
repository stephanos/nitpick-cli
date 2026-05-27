public struct HostAttentionSnapshot: Decodable, Equatable {
    public var kind: String
    public var title: String
    public var detail: String
    public var retryableActivityCount: Int

    public init(kind: String, title: String, detail: String, retryableActivityCount: Int) {
        self.kind = kind
        self.title = title
        self.detail = detail
        self.retryableActivityCount = retryableActivityCount
    }

    private enum CodingKeys: String, CodingKey {
        case kind
        case title
        case detail
        case retryableActivityCount = "retryable_activity_count"
    }
}

public struct HostStatus: Decodable, Equatable {
    public var activityCount: Int
    public var queuedActivityCount: Int
    public var runningActivityCount: Int
    public var completedActivityCount: Int
    public var errorActivityCount: Int
    public var openReviewCount: Int
    public var queuedReviewCount: Int
    public var runningReviewCount: Int
    public var artifactCount: Int
    public var localOnlyArtifactCount: Int
    public var pendingSyncArtifactCount: Int
    public var provider: String
    public var model: String?
    public var reviewSourceName: String
    public var reviewSourceEnabled: Bool
    public var reviewSourceLastPollUnix: UInt64?
    public var reviewSourceLastPollSummary: String?
    public var attention: HostAttentionSnapshot?

    private enum CodingKeys: String, CodingKey {
        case activityCount = "activity_count"
        case queuedActivityCount = "queued_activity_count"
        case runningActivityCount = "running_activity_count"
        case completedActivityCount = "completed_activity_count"
        case errorActivityCount = "error_activity_count"
        case openReviewCount = "open_review_count"
        case queuedReviewCount = "queued_review_count"
        case runningReviewCount = "running_review_count"
        case artifactCount = "artifact_count"
        case localOnlyArtifactCount = "local_only_artifact_count"
        case pendingSyncArtifactCount = "pending_sync_artifact_count"
        case provider
        case model
        case reviewSourceName = "review_source_name"
        case reviewSourceEnabled = "review_source_enabled"
        case reviewSourceLastPollUnix = "review_source_last_poll_unix"
        case reviewSourceLastPollSummary = "review_source_last_poll_summary"
        case attention
    }
}
