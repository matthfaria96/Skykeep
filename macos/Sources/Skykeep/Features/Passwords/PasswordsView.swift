import SwiftUI

struct PasswordsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let kc = model.keychain
        if kc.unlocked {
            PasswordsScreen()
        } else {
            VStack(spacing: 18) {
                ZStack {
                    Circle().fill(RadialGradient(colors: [Color.green.opacity(0.28), .clear], center: .center, startRadius: 10, endRadius: 90))
                        .frame(width: 180, height: 180)
                    Image(systemName: "touchid").font(.system(size: 64, weight: .light)).foregroundStyle(.green)
                }
                .padding(.bottom, -20)
                Text("Senhas trancadas").font(.system(size: 30, weight: .bold))
                Text("Use o Touch ID ou a senha do Mac para abrir.").font(.title3).foregroundStyle(.secondary)
                Button { Task { await kc.unlock() } } label: { Label("Desbloquear", systemImage: "lock.open.fill").padding(.horizontal, 8) }
                    .buttonStyle(.glassProminent).tint(.green).controlSize(.extraLarge)
                    .keyboardShortcut(.defaultAction)
                if let e = kc.error { Text(e).font(.caption).foregroundStyle(.red) }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

private enum Pane: Hashable { case browsers, generator, item(String) }

private struct PasswordsScreen: View {
    @Environment(AppModel.self) private var model
    @State private var pane: Pane = .browsers
    @State private var query = ""
    @State private var adding = false

    var body: some View {
        let kc = model.keychain
        let items = query.isEmpty ? kc.items : kc.items.filter { $0.title.localizedStandardContains(query) || $0.account.localizedStandardContains(query) }
        HStack(spacing: 0) {
            SourceColumn(width: 280) {
                SearchField(prompt: "Buscar no Chaveiro do Mac", text: $query)
                    .padding(.horizontal, 12).padding(.top, 12).padding(.bottom, 4)
                ScrollView {
                    VStack(alignment: .leading, spacing: 2) {
                        SourceRow(title: "Navegadores", symbol: "globe", tint: .green, selected: pane == .browsers) { pane = .browsers }
                        SourceRow(title: "Gerador de senhas", symbol: "wand.and.stars", tint: .green, selected: pane == .generator) { pane = .generator }
                        SourceHeading(text: "Chaveiro do Mac · \(items.count)")
                        ForEach(items) { item in
                            let selected = pane == .item(item.id)
                            Button { pane = .item(item.id) } label: {
                                HStack(spacing: 10) {
                                    SiteBadge(host: item.server)
                                    VStack(alignment: .leading, spacing: 1) {
                                        Text(item.title).lineLimit(1)
                                        Text(item.account).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                                    }
                                    Spacer()
                                }
                                .padding(.horizontal, 8).padding(.vertical, 5)
                                .background(selected ? Color.green.opacity(0.16) : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                        if items.isEmpty {
                            Text(query.isEmpty ? "As senhas do Chaveiro do iCloud ficam no app Senhas da Apple." : "Nada encontrado.")
                                .font(.caption).foregroundStyle(.secondary).padding(9)
                        }
                    }
                    .padding(10)
                }
            }
            Group {
                switch pane {
                case .browsers: BrowsersPane()
                case .generator: GeneratorPane()
                case .item(let id):
                    if let item = kc.items.first(where: { $0.id == id }) {
                        PasswordDetail(item: item).id(id)
                    } else {
                        QuietState(symbol: "key", title: "Nenhuma senha selecionada", tint: .green)
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .toolbar {
            ToolbarItemGroup {
                Button { adding = true } label: { Label("Nova senha", systemImage: "plus") }.help("Nova senha (⌘N)")
                Button { kc.lock() } label: { Label("Trancar", systemImage: "lock.fill") }.help("Trancar senhas")
            }
        }
        .sheet(isPresented: $adding) { AddPasswordSheet() }
        .onChange(of: model.newItemRequest) { adding = true }
    }
}

struct SiteBadge: View {
    let host: String
    var size: CGFloat = 26

    var body: some View {
        let letter = host.replacingOccurrences(of: "www.", with: "").prefix(1).uppercased()
        Text(letter.isEmpty ? "•" : letter)
            .font(.system(size: size * 0.45, weight: .semibold, design: .rounded))
            .foregroundStyle(.white)
            .frame(width: size, height: size)
            .background(Avatar.color(for: host).gradient, in: RoundedRectangle(cornerRadius: size * 0.25, style: .continuous))
    }
}

private struct PasswordDetail: View {
    @Environment(AppModel.self) private var model
    let item: KeychainItem
    @State private var password: String?
    @State private var reveal = false
    @State private var confirmDelete = false

    var body: some View {
        Form {
            Section {
                HStack(spacing: 12) {
                    SiteBadge(host: item.server, size: 44)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(item.title).font(.title2.bold())
                        if let m = item.modified { Text("Modificada em \(Fmt.short(m))").foregroundStyle(.secondary) }
                    }
                }
                .padding(.vertical, 4)
            }
            Section {
                LabeledContent("Usuário") {
                    HStack {
                        Text(item.account).textSelection(.enabled)
                        Button { Clipboard.copy(item.account) } label: { Image(systemName: "doc.on.doc") }
                            .buttonStyle(.borderless).help("Copiar usuário")
                    }
                }
                LabeledContent("Senha") {
                    HStack {
                        Text(reveal ? (password ?? "…") : "••••••••••••").font(.body.monospaced()).textSelection(.enabled)
                        Button(reveal ? "Ocultar" : "Mostrar") {
                            if reveal { reveal = false } else { Task { await load(); reveal = password != nil } }
                        }
                        .buttonStyle(.borderless)
                        Button { Task { await load(); if let p = password { Clipboard.copySecret(p) } } } label: { Image(systemName: "doc.on.doc") }
                            .buttonStyle(.borderless)
                            .help("Copiar senha (sai da área de transferência em 90 segundos)")
                    }
                }
                LabeledContent("Site") { Text(item.server).textSelection(.enabled) }
            }
            Section {
                Button("Apagar Senha…", role: .destructive) { confirmDelete = true }
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .confirmationDialog("Apagar a senha de \(item.title)?", isPresented: $confirmDelete) {
            Button("Apagar", role: .destructive) { Task { await model.keychain.delete(item) } }
        } message: {
            Text("Ela será removida do Chaveiro deste Mac.")
        }
    }

    private func load() async {
        if password == nil { password = await model.keychain.password(for: item) }
    }
}

private struct GeneratorPane: View {
    @State private var appleStyle = true
    @State private var length: Double = 20
    @State private var symbols = true
    @State private var value = PasswordGenerator.apple()
    @State private var copied = false

    var body: some View {
        Form {
            Section {
                HStack {
                    Text(value)
                        .font(.system(.title2, design: .monospaced))
                        .textSelection(.enabled)
                        .contentTransition(.numericText())
                    Spacer()
                    Button { regenerate() } label: { Image(systemName: "arrow.clockwise") }
                        .buttonStyle(.borderless).help("Gerar outra")
                    Button(copied ? "Copiada" : "Copiar") {
                        Clipboard.copySecret(value)
                        copied = true
                        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { copied = false }
                    }
                }
                .padding(.vertical, 6)
            } footer: {
                Text("A senha copiada sai da área de transferência em 90 segundos.")
            }
            Section("Opções") {
                Picker("Estilo", selection: $appleStyle) {
                    Text("Fácil de digitar (como a Apple sugere)").tag(true)
                    Text("Aleatória").tag(false)
                }
                if !appleStyle {
                    LabeledContent("Tamanho") {
                        HStack {
                            Slider(value: $length, in: 12...64, step: 1)
                            Text("\(Int(length))").monospacedDigit().frame(width: 28)
                        }
                    }
                    Toggle("Símbolos", isOn: $symbols)
                }
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .frame(maxWidth: 640)
        .frame(maxWidth: .infinity)
        .onChange(of: appleStyle) { regenerate() }
        .onChange(of: length) { regenerate() }
        .onChange(of: symbols) { regenerate() }
    }

    private func regenerate() {
        withAnimation(.snappy) {
            value = appleStyle ? PasswordGenerator.apple() : PasswordGenerator.random(length: Int(length), symbols: symbols)
        }
    }
}

private struct AddPasswordSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var server = ""
    @State private var account = ""
    @State private var password = PasswordGenerator.apple()
    @State private var error: String?

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    TextField("Site", text: $server, prompt: Text("exemplo.com"))
                    TextField("Usuário", text: $account, prompt: Text("voce@exemplo.com"))
                    HStack {
                        TextField("Senha", text: $password).font(.body.monospaced())
                        Button { password = PasswordGenerator.apple() } label: { Image(systemName: "wand.and.stars") }
                            .buttonStyle(.borderless).help("Gerar senha forte")
                    }
                } header: {
                    Text("Nova Senha")
                } footer: {
                    Text(error ?? "Fica guardada no Chaveiro deste Mac.").foregroundStyle(error == nil ? Color.secondary : Color.red)
                }
            }
            .formStyle(.grouped)
            HStack {
                Spacer()
                Button("Cancelar") { dismiss() }.keyboardShortcut(.cancelAction)
                Button("Salvar") {
                    Task {
                        if let e = await model.keychain.add(server: server, account: account, password: password) { error = e } else { dismiss() }
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(server.isEmpty || password.isEmpty)
            }
            .padding()
        }
        .frame(width: 440)
    }
}
