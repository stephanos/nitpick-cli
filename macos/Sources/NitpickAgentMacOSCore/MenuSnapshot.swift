public struct MenuSnapshot: Equatable {
    public var hostIsRunning: Bool
    public var activityCount: Int
    public var runningActivityCount: Int
    public var artifactCount: Int
    public var localOnlyArtifactCount: Int
    public var pendingSyncArtifactCount: Int

    public init(
        hostIsRunning: Bool,
        activityCount: Int,
        runningActivityCount: Int = 0,
        artifactCount: Int = 0,
        localOnlyArtifactCount: Int = 0,
        pendingSyncArtifactCount: Int = 0
    ) {
        self.hostIsRunning = hostIsRunning
        self.activityCount = activityCount
        self.runningActivityCount = runningActivityCount
        self.artifactCount = artifactCount
        self.localOnlyArtifactCount = localOnlyArtifactCount
        self.pendingSyncArtifactCount = pendingSyncArtifactCount
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
