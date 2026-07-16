import AutophagyKit
import SwiftUI

/// The standard Preferences window. One preference today: whether the app hides
/// its Dock icon and runs as a menu-bar-only accessory. The default is a normal
/// Dock application; the menu-bar extra is always present regardless.
struct SettingsView: View {
    // Backed by the same UserDefaults key as `AppSettings.menuBarOnly` so the
    // toggle and the activation-policy reader stay in sync.
    @AppStorage("menuBarOnly") private var menuBarOnly = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Appearance")
                .font(.headline)
            Toggle("Run as a menu-bar-only app (hide the Dock icon)", isOn: $menuBarOnly)
            Text("The menu-bar extra is always available. When this is on, the "
                + "app keeps running in the menu bar with no Dock icon; reopen "
                + "the main window from the menu bar's \u{201C}Open Autophagy.\u{201D}")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer()
        }
        .padding(20)
        .frame(width: 440, height: 180)
        .onChange(of: menuBarOnly) { _ in
            ActivationPolicy.apply()
        }
    }
}
