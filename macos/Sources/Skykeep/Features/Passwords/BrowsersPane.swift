import SwiftUI

/// One set of passwords for Safari, Vivaldi, Chrome, Brave, Edge, Arc… — the Passwords app's.
struct BrowsersPane: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let bridge = model.browsers
        Form {
            Section {
                SectionHeader(title: "Senhas em todos os navegadores", eyebrow: "Uma senha só, em todo lugar", tint: .green)
                Text("Conecte cada navegador ao app Senhas da Apple. Ele passa a preencher e salvar direto no app Senhas: o que você salvar no iPhone aparece no navegador do Mac, e vice-versa.")
                    .foregroundStyle(.secondary)
            }

            if let file = bridge.pendingFile {
                Section {
                    LabeledContent("\(bridge.pendingCount) senhas do \(bridge.pendingBrowser)", value: "prontas para importar")
                    Text("1. No app Senhas, escolha Arquivo › Importar Senhas…\n2. Selecione “\(file.lastPathComponent)” em Downloads.\n3. Volte aqui e apague o arquivo. Ele se apaga sozinho em 15 minutos.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                    HStack {
                        Button("Abrir o App Senhas") { KeychainService.openPasswordsApp() }
                        Button("Mostrar Arquivo") { bridge.showPendingFile() }
                        Spacer()
                        Button("Apagar Arquivo") { bridge.deletePendingFile() }
                            .buttonStyle(.borderedProminent)
                    }
                } header: {
                    Text("Importação")
                }
            }

            if let msg = bridge.message {
                Section { Label(msg, systemImage: "exclamationmark.triangle").foregroundStyle(.orange) }
            }

            Section("Navegadores") {
                if bridge.safariInstalled {
                    LabeledContent {
                        Label("Usa o app Senhas", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                    } label: {
                        browserLabel("Safari", NSWorkspace.shared.icon(forFile: "/Applications/Safari.app"))
                    }
                }
                ForEach(bridge.browsers) { b in BrowserRow(browser: b) }
                if bridge.browsers.isEmpty {
                    Text("Vivaldi, Chrome, Brave, Edge, Arc e Opera aparecem aqui quando instalados.").foregroundStyle(.secondary)
                }
            }

            Section {
                Text("A Apple distribui a extensão “Senhas do iCloud” para navegadores baseados no Chrome, mas o macOS só avisa o Chrome de onde fica o ajudante que liga a extensão ao app Senhas. Conectar grava esse mesmo aviso na pasta do navegador. Nenhuma senha passa pelo Skykeep nesse caminho.")
                Text("Depois de conectar, desligue “Salvar senhas” nos ajustes do próprio navegador para ele não guardar uma segunda cópia.")
            } header: {
                Text("Como Funciona")
            }
            .font(.callout)
            .foregroundStyle(.secondary)
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .onAppear { bridge.scan() }
    }
}

private func browserLabel(_ name: String, _ icon: NSImage) -> some View {
    HStack(spacing: 10) {
        Image(nsImage: icon).resizable().frame(width: 28, height: 28)
        Text(name)
    }
}

private struct BrowserRow: View {
    @Environment(AppModel.self) private var model
    let browser: BrowserInfo

    var body: some View {
        let bridge = model.browsers
        let st = bridge.status[browser.id] ?? BrowserStatus()
        VStack(alignment: .leading, spacing: 8) {
            LabeledContent {
                if st.connected && st.extensionInstalled {
                    Label("Conectado", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                } else if st.connected {
                    Label("Falta a extensão", systemImage: "puzzlepiece.extension").foregroundStyle(.orange)
                } else {
                    Text("Não conectado").foregroundStyle(.secondary)
                }
            } label: {
                browserLabel(browser.name, browser.icon)
            }
            HStack {
                if !st.connected {
                    Button("Conectar ao App Senhas") { bridge.connect(browser) }.buttonStyle(.glassProminent).tint(.green)
                }
                if !st.extensionInstalled {
                    Button("Instalar Extensão…") { bridge.installExtension(browser) }
                        .help("Abre a página da extensão Senhas do iCloud no \(browser.name)")
                }
                if bridge.restartNeeded.contains(browser.id) {
                    Button("Reiniciar \(browser.name)") { bridge.restart(browser) }
                }
                if st.savedPasswords > 0 {
                    if bridge.working == browser.id {
                        ProgressView().controlSize(.small)
                    } else {
                        Button("Importar \(st.savedPasswords) Senhas…") { Task { await bridge.prepareImport(browser) } }
                            .help("Leva as senhas salvas no \(browser.name) para o app Senhas")
                    }
                }
                Spacer()
                if st.connected && !st.connectedBySystem {
                    Button("Desconectar") { bridge.disconnect(browser) }.buttonStyle(.link)
                }
            }
            .controlSize(.small)
            if bridge.restartNeeded.contains(browser.id) {
                Text("Reinicie o \(browser.name) para a conexão valer. As abas voltam como estavam.").font(.caption).foregroundStyle(.secondary)
            } else if st.connected && st.extensionInstalled {
                Text("Na primeira vez, clique na chave da extensão na barra do \(browser.name) e digite o código que o Mac mostrar.").font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 4)
    }
}
