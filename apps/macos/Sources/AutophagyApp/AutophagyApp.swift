import AppKit
import AutophagyKit
import SwiftUI

/// The read-only macOS inspector for a local Autophagy database.
@main
struct AutophagyApp: App {
    @StateObject private var model = AppModel()
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    /// The main-window scene identifier, so the menu-bar extra can reopen it.
    static let mainWindowID = "main"

    var body: some Scene {
        WindowGroup("Autophagy", id: Self.mainWindowID) {
            RootView()
                .environmentObject(model)
                .frame(minWidth: 900, minHeight: 560)
                .onAppear { model.openStartupDatabaseIfAvailable() }
        }
        .commands {
            CommandGroup(replacing: .newItem) {}
        }

        // Always-available, read-only menu-bar presence. Uses the window style so
        // it can render quick stats and the recent-candidate list; a plain menu
        // cannot. The app keeps running here even when the main window is closed.
        MenuBarExtra("Autophagy", systemImage: "circle.hexagongrid.fill") {
            MenuBarView()
                .environmentObject(model)
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsView()
        }
    }
}

/// Applies the Dock-icon (activation-policy) preference at launch. The default
/// is a normal Dock application; the user may opt into a menu-bar-only app.
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_: Notification) {
        ActivationPolicy.apply()
    }
}

/// Maps the `menuBarOnly` preference onto the process activation policy.
@MainActor
enum ActivationPolicy {
    static func apply(settings: AppSettings = AppSettings()) {
        NSApplication.shared.setActivationPolicy(settings.menuBarOnly ? .accessory : .regular)
    }
}

/// Shows onboarding until a database is open, then the main navigation.
struct RootView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        if model.hasOpenDatabase {
            MainView()
        } else {
            OnboardingView()
        }
    }
}
