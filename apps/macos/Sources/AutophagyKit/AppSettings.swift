import Foundation

/// App-side preferences, stored in `UserDefaults` only.
///
/// Like ``DatabaseSelection`` these are viewer preferences; nothing is written
/// into the repository or the database. The only setting today controls whether
/// the app hides its Dock icon and runs as a menu-bar-only (accessory) app.
public struct AppSettings {
    private static let menuBarOnlyKey = "menuBarOnly"
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    /// Whether the app should run as a menu-bar-only accessory (no Dock icon).
    ///
    /// Defaults to `false`: the app is a normal Dock application unless the user
    /// opts in. Either way the menu-bar extra is always present.
    public var menuBarOnly: Bool {
        get { defaults.bool(forKey: Self.menuBarOnlyKey) }
        nonmutating set { defaults.set(newValue, forKey: Self.menuBarOnlyKey) }
    }
}
