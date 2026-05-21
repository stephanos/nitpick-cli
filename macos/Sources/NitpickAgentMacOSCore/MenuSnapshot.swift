import Foundation

public struct MenuSnapshot: Equatable {
    public var hostIsRunning: Bool
    public var activityCount: Int
    public var runningActivityCount: Int
    public var queuedReviewCount: Int
    public var runningReviewCount: Int
    public var artifactCount: Int
    public var localOnlyArtifactCount: Int
    public var pendingSyncArtifactCount: Int
    public var reviewSourceEnabled: Bool
    public var reviewSourceLastPollUnix: UInt64?
    public var reviewSourceLastPollSummary: String?
    public var statusIssue: MenuStatusIssue?
    public var activities: [ActivitySnapshot]
    public var currentUnix: UInt64

    public init(
        hostIsRunning: Bool,
        activityCount: Int,
        runningActivityCount: Int = 0,
        queuedReviewCount: Int = 0,
        runningReviewCount: Int = 0,
        artifactCount: Int = 0,
        localOnlyArtifactCount: Int = 0,
        pendingSyncArtifactCount: Int = 0,
        reviewSourceEnabled: Bool = false,
        reviewSourceLastPollUnix: UInt64? = nil,
        reviewSourceLastPollSummary: String? = nil,
        statusIssue: MenuStatusIssue? = nil,
        activities: [ActivitySnapshot] = [],
        currentUnix: UInt64 = UInt64(Date().timeIntervalSince1970)
    ) {
        self.hostIsRunning = hostIsRunning
        self.activityCount = activityCount
        self.runningActivityCount = runningActivityCount
        self.queuedReviewCount = queuedReviewCount
        self.runningReviewCount = runningReviewCount
        self.artifactCount = artifactCount
        self.localOnlyArtifactCount = localOnlyArtifactCount
        self.pendingSyncArtifactCount = pendingSyncArtifactCount
        self.reviewSourceEnabled = reviewSourceEnabled
        self.reviewSourceLastPollUnix = reviewSourceLastPollUnix
        self.reviewSourceLastPollSummary = reviewSourceLastPollSummary
        self.statusIssue = statusIssue
        self.activities = activities
        self.currentUnix = currentUnix
    }

    public var statusSummary: String {
        if let statusIssue {
            return statusIssue.title
        }
        guard hostIsRunning else {
            return "agent stopped"
        }
        var parts: [String] = []
        if runningReviewCount > 0 {
            parts.append("\(runningReviewCount) running")
        }
        if queuedReviewCount > 0 {
            parts.append("\(queuedReviewCount) queued")
        }
        if parts.isEmpty {
            return reviewSourceEnabled ? "idle" : "idle · discovery off"
        }
        return parts.joined(separator: " · ")
    }

    public var statusTitle: String {
        if let statusIssue {
            return statusIssue.title
        }
        guard hostIsRunning else {
            return "status: agent stopped"
        }
        if runningActivityCount == 1 {
            return artifactSuffix("status: 1 running")
        }
        if runningActivityCount > 1 {
            return artifactSuffix("status: \(runningActivityCount) running")
        }

        if !reviewSourceEnabled {
            return "status: discovery disabled"
        }

        return artifactSuffix("status: idle")
    }

    public var statusDetails: String? {
        statusIssue?.details
    }

    public var lastDiscoveryRefreshTitle: String? {
        guard hostIsRunning else {
            return nil
        }
        guard reviewSourceEnabled else {
            return "last discovery: disabled"
        }
        guard let reviewSourceLastPollUnix else {
            return "last discovery: never"
        }
        return "last discovery: \(relativeTime(reviewSourceLastPollUnix))"
    }

    public var recentActivityEntries: [ActivityMenuEntry] {
        activities
            .sorted { lhs, rhs in
                if lhs.updatedAtUnix == rhs.updatedAtUnix {
                    return lhs.id > rhs.id
                }
                return lhs.updatedAtUnix > rhs.updatedAtUnix
            }
            .prefix(5)
            .map { activity in
                ActivityMenuEntry(id: activity.id, title: activityTitle(activity))
            }
    }

    public var ongoingReviewEntries: [ActivityMenuEntry] {
        let activeReviews = activities
            .filter { activity in
                activity.kind == "Review" && (activity.status == "Running" || activity.status == "Queued")
            }
            .sorted { lhs, rhs in
                if lhs.status != rhs.status {
                    return lhs.status == "Running"
                }
                if lhs.updatedAtUnix == rhs.updatedAtUnix {
                    return lhs.id > rhs.id
                }
                return lhs.updatedAtUnix > rhs.updatedAtUnix
            }
        let visible = activeReviews.prefix(5).map { activity in
            ActivityMenuEntry(id: activity.id, title: ongoingReviewTitle(activity))
        }
        let hiddenQueuedCount = activeReviews.dropFirst(5).filter { activity in
            activity.status == "Queued"
        }.count
        if hiddenQueuedCount == 0 {
            return Array(visible)
        }
        return Array(visible) + [ActivityMenuEntry(id: nil, title: "\(hiddenQueuedCount) more queued...")]
    }

    private func activityTitle(_ activity: ActivitySnapshot) -> String {
        let verb = activityVerb(activity)
        let label = activity.label ?? fallbackLabel(activity)
        if verb.isEmpty {
            return "\(relativeTime(activity.updatedAtUnix).padding(toLength: 8, withPad: " ", startingAt: 0)) \(label)"
        }
        return "\(relativeTime(activity.updatedAtUnix).padding(toLength: 8, withPad: " ", startingAt: 0)) \(verb) \(label)"
    }

    private func activityVerb(_ activity: ActivitySnapshot) -> String {
        if activity.status == "Completed", activity.label?.hasSuffix(" cleaned up") == true {
            return ""
        }
        if activity.kind == "Discovery", activity.status == "Completed" {
            return ""
        }

        switch activity.status {
        case "Running":
            return "started"
        case "Completed":
            return "finished"
        case "Error":
            return "failed"
        case "Cancelled":
            return "cancelled"
        default:
            return activity.status.lowercased()
        }
    }

    private func ongoingReviewTitle(_ activity: ActivitySnapshot) -> String {
        let label = activity.label ?? fallbackLabel(activity)
        switch activity.status {
        case "Running":
            return "Running \(label)"
        case "Queued":
            return "Queued \(label)"
        default:
            return "\(activity.status) \(label)"
        }
    }

    private func fallbackLabel(_ activity: ActivitySnapshot) -> String {
        switch activity.kind {
        case "Review":
            return "review"
        case "Chat":
            return "chat"
        case "Discovery":
            return "discovery"
        default:
            return activity.kind.lowercased()
        }
    }

    private func relativeTime(_ unix: UInt64) -> String {
        let seconds = currentUnix > unix ? currentUnix - unix : 0
        if seconds < 60 {
            return "\(seconds)s ago"
        }
        let minutes = seconds / 60
        if minutes < 60 {
            return "\(minutes)m ago"
        }
        let hours = minutes / 60
        if hours < 24 {
            return "\(hours)h ago"
        }
        return "\(hours / 24)d ago"
    }

    private func artifactSuffix(_ title: String) -> String {
        var parts: [String] = []
        if localOnlyArtifactCount > 0 {
            parts.append("\(localOnlyArtifactCount) local")
        }
        if pendingSyncArtifactCount > 0 {
            parts.append("\(pendingSyncArtifactCount) pending")
        }
        guard !parts.isEmpty else {
            return title
        }

        return "\(title), \(parts.joined(separator: ", "))"
    }
}
