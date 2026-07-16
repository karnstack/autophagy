import Foundation

/// Remembers the user's chosen database path.
///
/// This is an app-side preference only. It is stored in `UserDefaults` and is
/// never written into the repository or the database. Clearing it simply
/// returns the app to its first-run state.
public struct DatabaseSelection {
    private static let key = "selectedDatabasePath"
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    /// The remembered path, if any.
    public var selectedPath: String? {
        get { defaults.string(forKey: Self.key) }
        nonmutating set {
            if let newValue {
                defaults.set(newValue, forKey: Self.key)
            } else {
                defaults.removeObject(forKey: Self.key)
            }
        }
    }

    /// Resolve the path to open on launch: the remembered choice if it still
    /// exists, otherwise the CLI's default location if a database is present
    /// there, otherwise `nil` (first-run onboarding).
    public func resolveStartupPath(
        defaultExists: Bool = DatabaseLocator.defaultDatabaseExists(),
        fileExists: (String) -> Bool = { FileManager.default.isReadableFile(atPath: $0) }
    ) -> String? {
        if let selectedPath, fileExists(selectedPath) {
            return selectedPath
        }
        if defaultExists {
            return DatabaseLocator.defaultDatabaseURL().path
        }
        return nil
    }
}
