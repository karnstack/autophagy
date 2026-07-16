import AutophagyKit
import SwiftUI

/// The read-only macOS inspector for a local Autophagy database.
@main
struct AutophagyApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        WindowGroup("Autophagy") {
            RootView()
                .environmentObject(model)
                .frame(minWidth: 900, minHeight: 560)
                .onAppear { model.openStartupDatabaseIfAvailable() }
        }
        .commands {
            CommandGroup(replacing: .newItem) {}
        }
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
