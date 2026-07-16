import Foundation
import SQLite3

/// A failure raised while opening or reading the local database.
public enum SQLiteError: Error, CustomStringConvertible, Equatable {
    /// The database file could not be opened for reading.
    case open(String)
    /// A statement could not be prepared or evaluated.
    case query(String)

    public var description: String {
        switch self {
        case let .open(message): "cannot open database: \(message)"
        case let .query(message): "query failed: \(message)"
        }
    }
}

// SQLite wants to copy bound text/blob values rather than borrow them.
private let sqliteTransient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

/// A strictly read-only SQLite connection.
///
/// The connection is opened with `SQLITE_OPEN_READONLY` and immediately sets
/// `PRAGMA query_only = ON`, so the application has no path to mutate the
/// database even in the presence of a programming error. This type is a
/// reference type and is intentionally **not** `Sendable`: a connection is used
/// from a single actor (the main actor in the app, the test's thread in tests).
public final class Database {
    private let handle: OpaquePointer

    /// Open `path` for read-only access.
    ///
    /// - Throws: ``SQLiteError/open(_:)`` if the file is missing, unreadable, or
    ///   not a SQLite database.
    public init(readonlyPath path: String) throws {
        var handle: OpaquePointer?
        let flags = SQLITE_OPEN_READONLY | SQLITE_OPEN_NOMUTEX
        let status = sqlite3_open_v2(path, &handle, flags, nil)
        guard status == SQLITE_OK, let handle else {
            let message = handle.map { String(cString: sqlite3_errmsg($0)) } ?? "unknown error"
            if let handle { sqlite3_close_v2(handle) }
            throw SQLiteError.open(message)
        }
        self.handle = handle
        // Belt and braces: even a read-only connection is pinned to query-only
        // so no trigger, virtual table, or future code can write.
        do {
            try execute("PRAGMA query_only = ON;")
        } catch {
            sqlite3_close_v2(handle)
            throw error
        }
    }

    deinit {
        sqlite3_close_v2(handle)
    }

    /// Run a statement for its side effects (used only for `PRAGMA` here).
    public func execute(_ sql: String) throws {
        var error: UnsafeMutablePointer<CChar>?
        if sqlite3_exec(handle, sql, nil, nil, &error) != SQLITE_OK {
            let message = error.map { String(cString: $0) } ?? lastErrorMessage()
            sqlite3_free(error)
            throw SQLiteError.query(message)
        }
    }

    /// Whether a table or view of the given name exists.
    public func objectExists(_ name: String) -> Bool {
        (try? queryScalarInt(
            "SELECT count(*) FROM sqlite_master WHERE type IN ('table','view') AND name = ?1;",
            text: [name]
        )).map { $0 > 0 } ?? false
    }

    /// Evaluate `sql`, mapping each row with `map`, returning all rows.
    ///
    /// String parameters are bound positionally from `text`.
    public func query<T>(
        _ sql: String,
        text parameters: [String] = [],
        map: (Row) throws -> T
    ) throws -> [T] {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(handle, sql, -1, &statement, nil) == SQLITE_OK else {
            throw SQLiteError.query(lastErrorMessage())
        }
        defer { sqlite3_finalize(statement) }

        for (index, value) in parameters.enumerated() {
            let position = Int32(index + 1)
            if sqlite3_bind_text(statement, position, value, -1, sqliteTransient) != SQLITE_OK {
                throw SQLiteError.query(lastErrorMessage())
            }
        }

        var results: [T] = []
        while true {
            switch sqlite3_step(statement) {
            case SQLITE_ROW:
                results.append(try map(Row(statement: statement)))
            case SQLITE_DONE:
                return results
            default:
                throw SQLiteError.query(lastErrorMessage())
            }
        }
    }

    /// Convenience for a single integer scalar (e.g. `count(*)`).
    public func queryScalarInt(_ sql: String, text parameters: [String] = []) throws -> Int {
        try query(sql, text: parameters) { $0.int(0) ?? 0 }.first ?? 0
    }

    private func lastErrorMessage() -> String {
        String(cString: sqlite3_errmsg(handle))
    }
}

/// A read cursor over one result row. Column access is by index.
public struct Row {
    private let statement: OpaquePointer?

    init(statement: OpaquePointer?) {
        self.statement = statement
    }

    /// Text column, or `nil` when the column is `NULL`.
    public func string(_ index: Int32) -> String? {
        guard sqlite3_column_type(statement, index) != SQLITE_NULL,
              let bytes = sqlite3_column_text(statement, index)
        else { return nil }
        return String(cString: bytes)
    }

    /// Integer column, or `nil` when the column is `NULL`.
    public func int(_ index: Int32) -> Int? {
        guard sqlite3_column_type(statement, index) != SQLITE_NULL else { return nil }
        return Int(sqlite3_column_int64(statement, index))
    }
}
