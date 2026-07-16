import Foundation

/// Resolves the default local database path.
///
/// This must match the path the `autophagy` CLI resolves. The CLI uses the Rust
/// `directories` crate (`ProjectDirs::from("sh", "autophagy", "Autophagy")`) and
/// joins `autophagy.db` onto its `data_local_dir`. On macOS that directory is
/// `~/Library/Application Support/sh.autophagy.Autophagy`, so the default file
/// is `~/Library/Application Support/sh.autophagy.Autophagy/autophagy.db`.
public enum DatabaseLocator {
    /// The reverse-DNS project directory name shared with the CLI.
    public static let projectDirectoryName = "sh.autophagy.Autophagy"

    /// The database file name shared with the CLI.
    public static let databaseFileName = "autophagy.db"

    /// The default database URL for the current user on macOS.
    ///
    /// - Parameter home: overridable home directory (used by tests).
    public static func defaultDatabaseURL(
        home: URL = FileManager.default.homeDirectoryForCurrentUser
    ) -> URL {
        home
            .appendingPathComponent("Library", isDirectory: true)
            .appendingPathComponent("Application Support", isDirectory: true)
            .appendingPathComponent(projectDirectoryName, isDirectory: true)
            .appendingPathComponent(databaseFileName, isDirectory: false)
    }

    /// Whether a readable file exists at the default location.
    public static func defaultDatabaseExists(
        home: URL = FileManager.default.homeDirectoryForCurrentUser
    ) -> Bool {
        FileManager.default.isReadableFile(atPath: defaultDatabaseURL(home: home).path)
    }
}
