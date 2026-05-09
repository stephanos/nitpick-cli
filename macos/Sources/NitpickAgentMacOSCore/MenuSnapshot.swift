import Foundation

public struct MenuSnapshot: Equatable {
    public var hostIsRunning: Bool
    public var activityCount: Int
    public var runningActivityCount: Int
    public var artifactCount: Int
    public var localOnlyArtifactCount: Int
    public var pendingSyncArtifactCount: Int
    public var activities: [ActivitySnapshot]
    public var currentUnix: UInt64

    public init(
        hostIsRunning: Bool,
        activityCount: Int,
        runningActivityCount: Int = 0,
        artifactCount: Int = 0,
        localOnlyArtifactCount: Int = 0,
        pendingSyncArtifactCount: Int = 0,
        activities: [ActivitySnapshot] = [],
        currentUnix: UInt64 = UInt64(Date().timeIntervalSince1970)
    ) {
        self.hostIsRunning = hostIsRunning
        self.activityCount = activityCount
        self.runningActivityCount = runningActivityCount
        self.artifactCount = artifactCount
        self.localOnlyArtifactCount = localOnlyArtifactCount
        self.pendingSyncArtifactCount = pendingSyncArtifactCount
        self.activities = activities
        self.currentUnix = currentUnix
    }

    public var statusTitle: String {
        guard hostIsRunning else {
            return "Status: Host stopped"
        }
        if runningActivityCount == 1 {
            return artifactSuffix("Status: 1 running")
        }
        if runningActivityCount > 1 {
            return artifactSuffix("Status: \(runningActivityCount) running")
        }

        switch activityCount {
        case 0:
            return "Status: Idle"
        case 1:
            return artifactSuffix("Status: 1 activity")
        default:
            return artifactSuffix("Status: \(activityCount) activities")
        }
    }

    public var recentActivityTitles: [String] {
        activities
            .sorted { lhs, rhs in
                if lhs.updatedAtUnix == rhs.updatedAtUnix {
                    return lhs.id > rhs.id
                }
                return lhs.updatedAtUnix > rhs.updatedAtUnix
            }
            .prefix(5)
            .map(activityTitle)
    }

    private func activityTitle(_ activity: ActivitySnapshot) -> String {
        "\(relativeTime(activity.updatedAtUnix).padding(toLength: 8, withPad: " ", startingAt: 0)) \(activityVerb(activity)) \(activity.label ?? fallbackLabel(activity))"
    }

    private func activityVerb(_ activity: ActivitySnapshot) -> String {
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

    private func fallbackLabel(_ activity: ActivitySnapshot) -> String {
        switch activity.kind {
        case "Review":
            return "review"
        case "Chat":
            return "chat"
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
