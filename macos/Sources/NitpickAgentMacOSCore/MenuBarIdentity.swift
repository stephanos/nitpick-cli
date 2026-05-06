public struct MenuBarIdentity: Equatable {
    public let symbolName: String
    public let fallbackTitle: String
    public let accessibilityDescription: String

    public init(
        symbolName: String = "text.badge.checkmark",
        fallbackTitle: String = "NP",
        accessibilityDescription: String = "Nitpick Agent"
    ) {
        self.symbolName = symbolName
        self.fallbackTitle = fallbackTitle
        self.accessibilityDescription = accessibilityDescription
    }
}
