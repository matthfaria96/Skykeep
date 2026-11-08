import ServiceManagement
import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @AppStorage("showMenuBar") private var showMenuBar = true
    @AppStorage("startSection") private var startSection = ""
    @State private var launchAtLogin = SMAppService.mainApp.status == .enabled

    var body: some View {
        TabView {
            Form {
                Picker("Abrir em", selection: $startSection) {
                    Text("Onde parei").tag("")
                    ForEach(AppSection.allCases) { Text($0.title).tag($0.rawValue) }
                }
                Toggle("Mostrar na barra de menus", isOn: $showMenuBar)
                Toggle("Abrir ao iniciar sessão", isOn: $launchAtLogin)
                    .onChange(of: launchAtLogin) { _, on in
                        do { if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() } }
                        catch { launchAtLogin = SMAppService.mainApp.status == .enabled }
                    }
            }
            .formStyle(.grouped)
            .tabItem { Label("Geral", systemImage: "gearshape") }

            Form {
                Section {
                    access("Calendário", ok: model.events.canReadEvents, anchor: "Privacy_Calendars")
                    access("Lembretes", ok: model.events.canReadReminders, anchor: "Privacy_Reminders")
                    access("Contatos", ok: model.contacts.isAuthorized, anchor: "Privacy_Contacts")
                    access("Fotos", ok: model.photos.isAuthorized, anchor: "Privacy_Photos")
                    LabeledContent("Notas (iCloud)") {
                        if model.notes.connected { Label("Conectado", systemImage: "checkmark.circle.fill").foregroundStyle(.green) }
                        else { Button("Entrar…") { model.go(.notes) } }
                    }
                    LabeledContent("E-mail (iCloud)") {
                        if model.mail.connected { Label(model.mail.email ?? "Conectado", systemImage: "checkmark.circle.fill").foregroundStyle(.green) }
                        else { Button("Conectar…") { model.go(.mail) } }
                    }
                    access("Imagens das notas (Acesso Total ao Disco)", ok: FullDiskAccess.granted, anchor: "Privacy_AllFiles")
                } header: {
                    Text("Acesso aos seus dados")
                } footer: {
                    Text("O Skykeep lê os dados que o próprio macOS já mantém sincronizados com o iCloud, sem abrir nenhum outro app. Nada sai do seu Mac por causa do Skykeep: sem contas, sem servidores, sem análises.")
                }
                Section("Segurança") {
                    LabeledContent("Senhas e álbum Ocultos", value: "Touch ID ou senha do Mac")
                    LabeledContent("Trancar de novo", value: "após 5 minutos fora do app")
                    LabeledContent("Senha copiada", value: "some em 90 segundos")
                }
            }
            .formStyle(.grouped)
            .tabItem { Label("Privacidade", systemImage: "hand.raised") }

            VStack(spacing: 10) {
                if let icon = NSApp.applicationIconImage {
                    Image(nsImage: icon).resizable().frame(width: 96, height: 96)
                }
                Text("Skykeep para Mac").font(.title2.bold())
                Text("Versão \(Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev")").foregroundStyle(.secondary)
                Text("Seu iCloud num só app — E-mail, Lembretes, Calendário, Notas, Contatos, Fotos, iCloud Drive e Senhas.")
                    .multilineTextAlignment(.center).foregroundStyle(.secondary).frame(maxWidth: 360)
                VStack(spacing: 2) {
                    Text("Feito por Matheus Faria").font(.callout.weight(.medium))
                    Link("github.com/matthfaria96/Skykeep", destination: URL(string: "https://github.com/matthfaria96/Skykeep")!).font(.caption)
                    Text("A partir do Skykeep de Yashwanth Kamireddi · Código aberto, GPL-3.0").font(.caption).foregroundStyle(.tertiary)
                }
                .padding(.top, 6)
            }
            .padding(30)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .tabItem { Label("Sobre", systemImage: "info.circle") }
        }
        .frame(width: 520, height: 420)
    }

    private func access(_ name: String, ok: Bool, anchor: String) -> some View {
        LabeledContent(name) {
            if ok {
                Label("Permitido", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
            } else {
                Button("Ajustes…") {
                    NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?\(anchor)")!)
                }
            }
        }
    }
}
