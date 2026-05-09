public struct MenuBarIdentity: Equatable {
    public let imageName: String
    public let symbolName: String
    public let fallbackTitle: String
    public let accessibilityDescription: String

    public init(
        imageName: String = "NitpickAgentMenuIcon",
        symbolName: String = "text.badge.checkmark",
        fallbackTitle: String = "NP",
        accessibilityDescription: String = "Nitpick Agent"
    ) {
        self.imageName = imageName
        self.symbolName = symbolName
        self.fallbackTitle = fallbackTitle
        self.accessibilityDescription = accessibilityDescription
    }
}
