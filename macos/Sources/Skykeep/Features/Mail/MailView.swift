import SwiftUI

struct MailView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let mail = model.mail
        switch mail.state {
        case .ready:
            MailScreen()
        case .idle, .loading:
            VStack(spacing: 12) {
                ProgressView()
                Text("Abrindo seu e-mail do iCloud…").foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .task { if mail.state == .idle { await mail.load() } }
        case .signedOut:
            MailSetup()
        case .failed(let msg):
            VStack(spacing: 14) {
                QuietState(symbol: "envelope.badge", title: "Não deu para abrir o e-mail", message: msg, tint: .blue)
                    .frame(maxHeight: 260)
                Button("Tentar de novo") { Task { await mail.load() } }.buttonStyle(.glass)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

/// Connecting iCloud Mail: the address and an app-specific password — Apple's own way of
/// letting a mail app other than Mail in. Kept in the Mac's keychain.
private struct MailSetup: View {
    @Environment(AppModel.self) private var model
    @State private var email = ""
    @State private var password = ""
    @State private var working = false
    @State private var error: String?

    var body: some View {
        VStack(spacing: 18) {
            ZStack {
                Circle()
                    .fill(RadialGradient(colors: [Color.blue.opacity(0.28), .clear], center: .center, startRadius: 10, endRadius: 90))
                    .frame(width: 170, height: 170)
                IconTile(symbol: "envelope.fill", tint: .blue, size: 72)
            }
            .padding(.bottom, -18)
            Text("Conectar o e-mail do iCloud").font(.system(size: 28, weight: .bold))
            Text("Ler, responder, enviar, arquivar e apagar daqui — tudo no servidor do iCloud, igual no iPhone.")
                .font(.title3).foregroundStyle(.secondary).multilineTextAlignment(.center).frame(maxWidth: 460)

            VStack(alignment: .leading, spacing: 8) {
                step(1, "Em account.apple.com, vá em Entrar e Segurança › Senhas de App e crie uma chamada “Skykeep”.")
                step(2, "Copie a senha que aparece (algo como abcd-efgh-ijkl-mnop) e cole abaixo.")
                Button { NSWorkspace.shared.open(URL(string: "https://account.apple.com/account/manage/section/security")!) } label: {
                    Label("Abrir account.apple.com", systemImage: "arrow.up.forward.square")
                }
                .buttonStyle(.link)
                .padding(.leading, 26)
            }
            .widget(tint: .blue, radius: 16, padding: 14)
            .frame(maxWidth: 440)

            VStack(spacing: 10) {
                TextField("seu@icloud.com", text: $email)
                    .textFieldStyle(.plain).font(.system(size: 15))
                    .padding(.horizontal, 14).padding(.vertical, 11)
                    .glassEffect(.regular, in: .rect(cornerRadius: 12))
                SecureField("Senha de app", text: $password)
                    .textFieldStyle(.plain).font(.system(size: 15))
                    .padding(.horizontal, 14).padding(.vertical, 11)
                    .glassEffect(.regular, in: .rect(cornerRadius: 12))
                    .onSubmit(connect)
            }
            .frame(maxWidth: 340)

            if let error {
                Label(error, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.orange).font(.callout)
                    .multilineTextAlignment(.center).frame(maxWidth: 420)
            }

            Button(action: connect) {
                HStack {
                    if working { ProgressView().controlSize(.small) }
                    Text("Conectar").padding(.horizontal, 10)
                }
            }
            .buttonStyle(.glassProminent).tint(.blue).controlSize(.extraLarge)
            .keyboardShortcut(.defaultAction)
            .disabled(working || email.isEmpty || password.isEmpty)

            Label("A senha de app fica no Chaveiro do Mac e só serve para o e-mail. Você pode revogá-la quando quiser em account.apple.com.", systemImage: "lock.fill")
                .font(.footnote).foregroundStyle(.tertiary).multilineTextAlignment(.center).frame(maxWidth: 440)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func step(_ n: Int, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text("\(n)").font(.caption.weight(.bold)).foregroundStyle(.white)
                .frame(width: 18, height: 18).background(Color.blue, in: Circle())
            Text(text).font(.callout)
        }
    }

    private func connect() {
        guard !working else { return }
        working = true
        error = nil
        Task {
            error = await model.mail.setup(email: email, password: password)
            working = false
        }
    }
}

struct ComposeDraft: Identifiable {
    let id = UUID()
    var to = ""
    var cc = ""
    var subject = ""
    var text = ""
    var inReplyTo: String?
}

private struct MailScreen: View {
    @Environment(AppModel.self) private var model
    @State private var query = ""
    @State private var compose: ComposeDraft?

    var body: some View {
        @Bindable var mail = model.mail
        let list = query.isEmpty ? mail.shown : mail.shown.filter {
            $0.subject.localizedStandardContains(query) || $0.senderName.localizedStandardContains(query)
                || $0.senderEmail.localizedStandardContains(query) || $0.summary.localizedStandardContains(query)
        }

        HStack(spacing: 0) {
            SourceColumn(width: 360) {
                VStack(spacing: 10) {
                    SectionHeader(title: mail.filter == .sent ? "Enviadas" : "Entrada",
                                  eyebrow: mail.filter == .sent ? (mail.email ?? "") : (mail.unread > 0 ? "\(mail.unread) não lida\(mail.unread == 1 ? "" : "s")" : "Tudo lido"),
                                  tint: .blue) {
                        if mail.refreshing { ProgressView().controlSize(.small) }
                    }
                    Picker("", selection: $mail.filter) {
                        ForEach(MailService.Filter.allCases, id: \.self) { Text($0 == .inbox ? "Todas" : $0.rawValue).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    SearchField(prompt: "Buscar", text: $query)
                }
                .padding(.horizontal, 16)
                .padding(.top, 14)
                .padding(.bottom, 8)

                List(selection: $mail.selectedID) {
                    ForEach(list) { m in
                        MailRow(message: m, sent: mail.filter == .sent)
                            .tag(m.id)
                            .listRowSeparator(.hidden)
                            .contextMenu {
                                Button(m.read ? "Marcar como não lida" : "Marcar como lida") { mail.setRead(m, !m.read) }
                                Button(m.flagged ? "Remover sinalização" : "Sinalizar") { mail.toggleFlag(m) }
                                Divider()
                                Button("Arquivar") { mail.archive(m) }
                                Button("Apagar", role: .destructive) { mail.delete(m) }
                            }
                    }
                }
                .listStyle(.sidebar)
                .overlay {
                    if list.isEmpty {
                        QuietState(symbol: mail.filter == .flagged ? "flag" : "tray", title: query.isEmpty ? "Nada aqui" : "Nada encontrado", tint: .blue, compact: true)
                    }
                }
                if let e = mail.lastError {
                    Label(e, systemImage: "exclamationmark.triangle").font(.caption).foregroundStyle(.orange).lineLimit(2).padding(10)
                }
            }

            Group {
                if let m = mail.message(mail.selectedID) {
                    MailReader(message: m, compose: { compose = $0 }).id("\(m.mailbox)/\(m.id)")
                } else {
                    QuietState(symbol: "envelope.open", title: "Nenhuma mensagem selecionada", message: "Escolha uma mensagem à esquerda.", tint: .blue)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .toolbar {
            ToolbarItemGroup {
                Button { Task { await mail.refresh() } } label: { Label("Atualizar", systemImage: "arrow.clockwise") }
                    .help("Buscar mensagens novas")
                Button { compose = ComposeDraft() } label: { Label("Nova mensagem", systemImage: "square.and.pencil") }
                    .help("Nova mensagem (⌘N)")
                Menu {
                    if let e = mail.email { Text(e) }
                    Button("Desconectar e-mail…", role: .destructive) { Task { await mail.signOut() } }
                } label: { Label("Conta", systemImage: "person.crop.circle") }
            }
        }
        .onChange(of: model.newItemRequest) { compose = ComposeDraft() }
        .onChange(of: mail.selectedID) { _, id in
            if let m = mail.message(id), !m.read { mail.setRead(m, true) }
        }
        .sheet(item: $compose) { draft in ComposeView(draft: draft) }
    }
}

private struct MailRow: View {
    let message: MailMessage
    var sent = false

    var body: some View {
        let m = message
        HStack(alignment: .top, spacing: 10) {
            ZStack(alignment: .topLeading) {
                Avatar(name: m.senderName, imageData: nil, size: 34)
                if !m.read {
                    Circle().fill(.blue).frame(width: 9, height: 9)
                        .overlay(Circle().strokeBorder(Color(nsColor: .windowBackgroundColor), lineWidth: 1.5))
                        .offset(x: -3, y: -2)
                }
            }
            VStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline) {
                    Text(m.senderName).font(.system(size: 13, weight: m.read ? .medium : .bold)).lineLimit(1)
                    Spacer(minLength: 6)
                    if m.flagged { Image(systemName: "flag.fill").font(.caption2).foregroundStyle(.orange) }
                    Text(Fmt.listDate(m.date)).font(.caption).foregroundStyle(.secondary)
                }
                Text(m.subject)
                    .font(.system(size: 12.5, weight: m.read ? .regular : .semibold))
                    .foregroundStyle(m.read ? .secondary : .primary)
                    .lineLimit(1)
                if !m.summary.isEmpty {
                    Text(m.summary).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                }
            }
        }
        .padding(.vertical, 5)
    }
}

private struct MailReader: View {
    @Environment(AppModel.self) private var model
    let message: MailMessage
    let compose: (ComposeDraft) -> Void
    @State private var body_: MailBody?
    @State private var loading = true

    var body: some View {
        let m = message
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 8) {
                    action("arrowshape.turn.up.left", "Responder") { reply(all: false) }
                    action("arrowshape.turn.up.left.2", "Responder a todos") { reply(all: true) }
                    action("arrowshape.turn.up.right", "Encaminhar") { forward() }
                    Spacer()
                    action(m.read ? "envelope.badge" : "envelope.open", m.read ? "Marcar como não lida" : "Marcar como lida") { model.mail.setRead(m, !m.read) }
                    action(m.flagged ? "flag.fill" : "flag", m.flagged ? "Remover sinalização" : "Sinalizar") { model.mail.toggleFlag(m) }
                    action("archivebox", "Arquivar") { model.mail.archive(m) }
                    action("trash", "Apagar") { model.mail.delete(m) }
                }
                Text(m.subject).font(.system(size: 22, weight: .bold)).textSelection(.enabled)
                HStack(spacing: 12) {
                    Avatar(name: m.senderName, imageData: nil, size: 40)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(m.senderName).font(.headline)
                        Text(m.senderEmail).font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
                    }
                    Spacer()
                    VStack(alignment: .trailing, spacing: 1) {
                        Text(Fmt.weekdayDay(m.date)).font(.callout)
                        Text(Fmt.time(m.date)).font(.caption).foregroundStyle(.secondary)
                    }
                }
                if let b = body_, !b.attachments.isEmpty {
                    HStack(spacing: 6) {
                        ForEach(b.attachments.prefix(4), id: \.self) { Pill(text: $0, symbol: "paperclip", tint: .blue) }
                        if b.attachments.count > 4 { Text("+\(b.attachments.count - 4)").font(.caption).foregroundStyle(.secondary) }
                    }
                }
            }
            .padding(.horizontal, 28)
            .padding(.top, 14)
            .padding(.bottom, 16)
            Divider().opacity(0.6)
            Group {
                if let b = body_ {
                    MailWebView(html: b.page)
                } else if loading {
                    ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    QuietState(symbol: "envelope.badge", title: "Não deu para abrir esta mensagem", message: "Tente atualizar a caixa.", tint: .blue)
                }
            }
        }
        .task {
            body_ = await model.mail.content(of: m)
            loading = false
        }
    }

    private func quoted(_ b: MailBody?) -> String {
        let m = message
        let original = (b?.quotable ?? "").split(separator: "\n", omittingEmptySubsequences: false).map { "> " + $0 }.joined(separator: "\n")
        return "\n\nEm \(Fmt.weekdayDay(m.date)), às \(Fmt.time(m.date)), \(m.senderName) <\(m.senderEmail)> escreveu:\n\n\(original)"
    }

    private func reply(all: Bool) {
        let m = message
        var d = ComposeDraft()
        d.to = body_?.replyTo ?? m.senderEmail
        if all, let b = body_ {
            let me = model.mail.email?.lowercased() ?? ""
            d.cc = (b.to + b.cc).filter { $0.lowercased() != me && $0.lowercased() != d.to.lowercased() }.joined(separator: ", ")
        }
        d.subject = m.subject.lowercased().hasPrefix("re:") ? m.subject : "Re: \(m.subject)"
        d.text = quoted(body_)
        d.inReplyTo = body_?.messageID
        compose(d)
    }

    private func forward() {
        let m = message
        var d = ComposeDraft()
        d.subject = m.subject.lowercased().hasPrefix("fwd:") ? m.subject : "Fwd: \(m.subject)"
        d.text = "\n\n---------- Mensagem encaminhada ----------\nDe: \(m.senderName) <\(m.senderEmail)>\nData: \(Fmt.weekdayDay(m.date)), \(Fmt.time(m.date))\nAssunto: \(m.subject)\n\n" + (body_?.quotable ?? "")
        compose(d)
    }

    private func action(_ symbol: String, _ help: String, _ act: @escaping () -> Void) -> some View {
        Button(action: act) {
            Image(systemName: symbol)
                .font(.system(size: 14, weight: .medium))
                .frame(width: 32, height: 30)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .glassEffect(.regular.interactive(), in: .capsule)
        .help(help)
    }
}

/// Writing a message: sent by iCloud Mail over SMTP, filed in Sent Messages.
struct ComposeView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State var draft: ComposeDraft
    @State private var sending = false
    @State private var error: String?
    @FocusState private var bodyFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(draft.inReplyTo != nil ? "Responder" : "Nova mensagem").font(.headline)
                Spacer()
                if let e = model.mail.email { Text("De: \(e)").font(.caption).foregroundStyle(.secondary) }
            }
            .padding(.horizontal, 18).padding(.vertical, 12)
            Divider()
            row("Para:", text: $draft.to, prompt: "nome@exemplo.com, outro@exemplo.com")
            Divider().padding(.leading, 18)
            row("Cc:", text: $draft.cc, prompt: "")
            Divider().padding(.leading, 18)
            row("Assunto:", text: $draft.subject, prompt: "")
            Divider()
            TextEditor(text: $draft.text)
                .font(.system(size: 14))
                .scrollContentBackground(.hidden)
                .padding(14)
                .focused($bodyFocused)
            Divider()
            HStack {
                if let error { Label(error, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.orange).font(.callout).lineLimit(2) }
                Spacer()
                Button("Cancelar") { dismiss() }.keyboardShortcut(.cancelAction)
                Button {
                    send()
                } label: {
                    HStack {
                        if sending { ProgressView().controlSize(.small) }
                        Label("Enviar", systemImage: "paperplane.fill")
                    }
                }
                .buttonStyle(.glassProminent).tint(.blue)
                .keyboardShortcut(.return, modifiers: [.command, .shift])
                .disabled(sending || addresses(draft.to).isEmpty)
            }
            .padding(14)
        }
        .frame(width: 640, height: 520)
        .onAppear { if draft.inReplyTo != nil { bodyFocused = true } }
    }

    private func row(_ label: String, text: Binding<String>, prompt: String) -> some View {
        HStack(spacing: 8) {
            Text(label).foregroundStyle(.secondary).frame(width: 64, alignment: .trailing)
            TextField("", text: text, prompt: Text(prompt)).textFieldStyle(.plain)
        }
        .padding(.horizontal, 18).padding(.vertical, 9)
    }

    private func addresses(_ s: String) -> [String] {
        s.split(whereSeparator: { $0 == "," || $0 == ";" }).map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    }

    private func send() {
        sending = true
        error = nil
        Task {
            if let e = await model.mail.send(to: addresses(draft.to), cc: addresses(draft.cc), subject: draft.subject, text: draft.text, inReplyTo: draft.inReplyTo) {
                error = e
                sending = false
            } else {
                dismiss()
            }
        }
    }
}
