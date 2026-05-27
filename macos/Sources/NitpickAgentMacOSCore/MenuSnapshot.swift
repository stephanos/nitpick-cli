import Foundation

public struct MenuSnapshot: Equatable {
    public var hostIsRunning: Bool
    public var activityCount: Int
    public var runningActivityCount: Int
    public var openReviewCount: Int
    public var queuedReviewCount: Int
    public var runningReviewCount: Int
    public var artifactCount: Int
    public var localOnlyArtifactCount: Int
    public var pendingSyncArtifactCount: Int
    public var reviewSourceEnabled: Bool
    public var reviewSourceLastPollUnix: UInt64?
    public var reviewSourceLastPollSummary: String?
    public var statusIssue: MenuStatusIssue?
    public var attention: HostAttentionSnapshot?
    public var activities: [ActivitySnapshot]
    public var currentUnix: UInt64

    public init(
        hostIsRunning: Bool,
        activityCount: Int,
        runningActivityCount: Int = 0,
        openReviewCount: Int = 0,
        queuedReviewCount: Int = 0,
        runningReviewCount: Int = 0,
        artifactCount: Int = 0,
        localOnlyArtifactCount: Int = 0,
        pendingSyncArtifactCount: Int = 0,
        reviewSourceEnabled: Bool = false,
        reviewSourceLastPollUnix: UInt64? = nil,
        reviewSourceLastPollSummary: String? = nil,
        statusIssue: MenuStatusIssue? = nil,
        attention: HostAttentionSnapshot? = nil,
        activities: [ActivitySnapshot] = [],
        currentUnix: UInt64 = UInt64(Date().timeIntervalSince1970)
    ) {
        self.hostIsRunning = hostIsRunning
        self.activityCount = activityCount
        self.runningActivityCount = runningActivityCount
        self.openReviewCount = openReviewCount
        self.queuedReviewCount = queuedReviewCount
        self.runningReviewCount = runningReviewCount
        self.artifactCount = artifactCount
        self.localOnlyArtifactCount = localOnlyArtifactCount
        self.pendingSyncArtifactCount = pendingSyncArtifactCount
        self.reviewSourceEnabled = reviewSourceEnabled
        self.reviewSourceLastPollUnix = reviewSourceLastPollUnix
        self.reviewSourceLastPollSummary = reviewSourceLastPollSummary
        self.statusIssue = statusIssue
        self.attention = attention
        self.activities = activities
        self.currentUnix = currentUnix
    }

    public var openReviewsSummary: String {
        MenuPresentation(snapshot: self).status.openReviewsTitle
    }

    public var statusTitle: String {
        MenuPresentation(snapshot: self).status.title
    }

    public var statusDetails: String? {
        MenuPresentation(snapshot: self).status.details
    }

    public var lastDiscoveryRefreshTitle: String? {
        let item = MenuPresentation(snapshot: self).lastDiscoveryRefresh
        return item.isHidden ? nil : item.title
    }

    public var recentActivityEntries: [ActivityMenuEntry] {
        MenuPresentation(snapshot: self).recentActivities.items
    }

    public var ongoingReviewEntries: [ActivityMenuEntry] {
        MenuPresentation(snapshot: self).ongoingReviews
    }
}
