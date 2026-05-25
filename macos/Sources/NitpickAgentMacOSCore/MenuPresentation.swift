import Foundation

public struct MenuStatusPresentation: Equatable {
    public let openReviewsTitle: String
    public let title: String
    public let details: String?
    public let agentErrorItem: ActivityMenuEntry

    public init(openReviewsTitle: String, title: String, details: String?) {
        self.openReviewsTitle = openReviewsTitle
        self.title = title
        self.details = details
        agentErrorItem = ActivityMenuEntry(
            id: nil,
            title: details == nil ? "" : title,
            isEnabled: details != nil,
            isHidden: details == nil
        )
    }
}

public struct MenuActivitySectionPresentation: Equatable {
    public let isHidden: Bool
    public let items: [ActivityMenuEntry]

    public init(items: [ActivityMenuEntry]) {
        isHidden = items.isEmpty
        self.items = items
    }
}

public struct MenuPresentation: Equatable {
    public let status: MenuStatusPresentation
    public let lastDiscoveryRefresh: ActivityMenuEntry
    public let ongoingReviews: [ActivityMenuEntry]
    public let recentActivities: MenuActivitySectionPresentation

    public init(snapshot: MenuSnapshot) {
        let statusTitle = Self.statusTitle(snapshot)
        let statusDetails = snapshot.statusIssue?.details
        status = MenuStatusPresentation(
            openReviewsTitle: Self.openReviewsTitle(snapshot),
            title: statusTitle,
            details: statusDetails
        )
        lastDiscoveryRefresh = Self.lastDiscoveryRefresh(snapshot)
        ongoingReviews = Self.ongoingReviewEntries(snapshot)
        recentActivities = MenuActivitySectionPresentation(items: Self.recentActivityEntries(snapshot))
    }

    private static func openReviewsTitle(_ snapshot: MenuSnapshot) -> String {
        if snapshot.openReviewCount == 0 {
            return "no open reviews"
        }
        if snapshot.openReviewCount == 1 {
            return "1 open review"
        }
        return "\(snapshot.openReviewCount) open reviews"
    }

    private static func statusTitle(_ snapshot: MenuSnapshot) -> String {
        if let statusIssue = snapshot.statusIssue {
            return statusIssue.title
        }
        guard snapshot.hostIsRunning else {
            return "status: agent stopped"
        }
        if snapshot.runningActivityCount == 1 {
            return artifactSuffix("status: 1 running", snapshot: snapshot)
        }
        if snapshot.runningActivityCount > 1 {
            return artifactSuffix("status: \(snapshot.runningActivityCount) running", snapshot: snapshot)
        }

        if !snapshot.reviewSourceEnabled {
            return "status: discovery disabled"
        }

        return artifactSuffix("status: idle", snapshot: snapshot)
    }

    private static func lastDiscoveryRefresh(_ snapshot: MenuSnapshot) -> ActivityMenuEntry {
        guard snapshot.hostIsRunning else {
            return ActivityMenuEntry(id: nil, title: "", isEnabled: false, isHidden: true)
        }
        guard snapshot.reviewSourceEnabled else {
            return ActivityMenuEntry(id: nil, title: "last discovery: disabled", isEnabled: false)
        }
        guard let reviewSourceLastPollUnix = snapshot.reviewSourceLastPollUnix else {
            return ActivityMenuEntry(id: nil, title: "last discovery: never", isEnabled: false)
        }
        return ActivityMenuEntry(
            id: nil,
            title: "last discovery: \(relativeTime(reviewSourceLastPollUnix, currentUnix: snapshot.currentUnix))",
            isEnabled: false
        )
    }

    private static func recentActivityEntries(_ snapshot: MenuSnapshot) -> [ActivityMenuEntry] {
        snapshot.activities
            .sorted { lhs, rhs in
                if lhs.updatedAtUnix == rhs.updatedAtUnix {
                    return lhs.id > rhs.id
                }
                return lhs.updatedAtUnix > rhs.updatedAtUnix
            }
            .prefix(5)
            .map { activity in
                ActivityMenuEntry(id: activity.id, title: activityTitle(activity, currentUnix: snapshot.currentUnix))
            }
    }

    private static func ongoingReviewEntries(_ snapshot: MenuSnapshot) -> [ActivityMenuEntry] {
        let activeReviews = snapshot.activities
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
        return Array(visible) + [
            ActivityMenuEntry(id: nil, title: "\(hiddenQueuedCount) more queued...", isEnabled: false),
        ]
    }

    private static func activityTitle(_ activity: ActivitySnapshot, currentUnix: UInt64) -> String {
        let verb = activityVerb(activity)
        let label = activity.label ?? fallbackLabel(activity)
        if verb.isEmpty {
            return "\(relativeTime(activity.updatedAtUnix, currentUnix: currentUnix).padding(toLength: 8, withPad: " ", startingAt: 0)) \(label)"
        }
        return "\(relativeTime(activity.updatedAtUnix, currentUnix: currentUnix).padding(toLength: 8, withPad: " ", startingAt: 0)) \(verb) \(label)"
    }

    private static func activityVerb(_ activity: ActivitySnapshot) -> String {
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

    private static func ongoingReviewTitle(_ activity: ActivitySnapshot) -> String {
        let label = activity.label ?? fallbackLabel(activity)
        switch activity.status {
        case "Running":
            return "🤖 Running \(label)"
        case "Queued":
            return "🤖 Queued \(label)"
        default:
            return "🤖 \(activity.status) \(label)"
        }
    }

    private static func fallbackLabel(_ activity: ActivitySnapshot) -> String {
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

    private static func relativeTime(_ unix: UInt64, currentUnix: UInt64) -> String {
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

    private static func artifactSuffix(_ title: String, snapshot: MenuSnapshot) -> String {
        var parts: [String] = []
        if snapshot.localOnlyArtifactCount > 0 {
            parts.append("\(snapshot.localOnlyArtifactCount) local")
        }
        if snapshot.pendingSyncArtifactCount > 0 {
            parts.append("\(snapshot.pendingSyncArtifactCount) pending")
        }
        guard !parts.isEmpty else {
            return title
        }

        return "\(title), \(parts.joined(separator: ", "))"
    }
}
