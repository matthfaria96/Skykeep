import SwiftUI

@main
struct SkykeepApp: App {
    @State private var model = AppModel()
    @AppStorage("showMenuBar") private var showMenuBar = true

    init() {
        // Launched straight from the build folder (no bundle), still behave as a regular app.
        NSApplication.shared.setActivationPolicy(.regular)
    }

    var body: some Scene {
        Window("Skykeep", id: "main") {
            RootView()
                .environment(model)
                .frame(minWidth: 1080, minHeight: 640)
        }
        .defaultSize(width: 1240, height: 780)
        .windowToolbarStyle(.unified)
        .commands { AppCommands(model: model) }

        Settings {
            SettingsView()
                .environment(model)
        }

        MenuBarExtra("Skykeep", systemImage: "cloud.fill", isInserted: $showMenuBar) {
            MenuBarView()
                .environment(model)
        }
        .menuBarExtraStyle(.window)
    }
}

struct AppCommands: Commands {
    let model: AppModel

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("Novo") { model.newItemRequest += 1 }
                .keyboardShortcut("n")
        }
        CommandGroup(after: .textEditing) {
            Button("Buscar em Tudo…") { model.showSearch = true }
                .keyboardShortcut("k")
        }
        CommandMenu("Ir") {
            ForEach(AppSection.allCases) { s in
                Button(s.title) { model.go(s) }
                    .keyboardShortcut(s.shortcut)
            }
        }
    }
}
