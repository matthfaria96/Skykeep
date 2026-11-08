import SwiftUI

/// Sign in to iCloud, as icloud.com does: the Apple Account and password (checked with Apple's
/// SRP — the password itself is never sent), then the six-digit code Apple shows on your
/// devices. The session is kept in this Mac's keychain, so this is asked once.
struct ICloudSignIn: View {
    @Environment(AppModel.self) private var model
    let section: AppSection
    let message: String

    @State private var appleID = ""
    @State private var password = ""
    @State private var code = ""
    @State private var working = false
    @State private var error: String?
    @FocusState private var focus: Field?

    enum Field { case id, password, code }

    private var needsCode: Bool { model.notes.state == .needsCode }

    var body: some View {
        VStack(spacing: 18) {
            ZStack {
                Circle()
                    .fill(RadialGradient(colors: [section.tint.opacity(0.28), .clear], center: .center, startRadius: 10, endRadius: 90))
                    .frame(width: 170, height: 170)
                IconTile(symbol: needsCode ? "lock.shield.fill" : "icloud.fill", tint: needsCode ? .blue : section.tint, size: 72)
            }
            .padding(.bottom, -18)

            Text(needsCode ? "Código de verificação" : "Entrar no iCloud").font(.system(size: 28, weight: .bold))
            Text(needsCode ? "Digite o código de 6 dígitos que apareceu no seu iPhone, iPad ou Mac." : message)
                .font(.title3)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 440)

            VStack(spacing: 10) {
                if needsCode {
                    TextField("000000", text: $code)
                        .font(.system(size: 28, weight: .semibold, design: .monospaced))
                        .multilineTextAlignment(.center)
                        .textFieldStyle(.plain)
                        .padding(.vertical, 10)
                        .frame(width: 220)
                        .glassEffect(.regular, in: .rect(cornerRadius: 14))
                        .focused($focus, equals: .code)
                        .onChange(of: code) { _, v in
                            let digits = String(v.filter(\.isNumber).prefix(6))
                            if digits != v { code = digits }
                            if digits.count == 6 { submit() }
                        }
                } else {
                    field("Conta Apple (e-mail ou telefone)", text: $appleID, secure: false, focus: .id)
                    field("Senha", text: $password, secure: true, focus: .password)
                }
            }
            .frame(maxWidth: 340)

            if let error {
                Label(error, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.orange).font(.callout)
                    .multilineTextAlignment(.center).frame(maxWidth: 420)
            }

            HStack(spacing: 10) {
                if needsCode {
                    Button("Reenviar código") { Task { await model.notes.resendCode() } }
                        .buttonStyle(.glass).controlSize(.large)
                }
                Button(action: submit) {
                    HStack {
                        if working { ProgressView().controlSize(.small) }
                        Text(needsCode ? "Verificar" : "Entrar").padding(.horizontal, 10)
                    }
                }
                .buttonStyle(.glassProminent).tint(section.tint).controlSize(.extraLarge)
                .keyboardShortcut(.defaultAction)
                .disabled(working || (needsCode ? code.count < 6 : (appleID.isEmpty || password.isEmpty)))
            }

            Label("A senha vai direto para a Apple, protegida como no icloud.com. Nada passa por servidores do Skykeep.", systemImage: "lock.fill")
                .font(.footnote)
                .foregroundStyle(.tertiary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
                .padding(.top, 4)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear { focus = needsCode ? .code : .id }
    }

    private func field(_ title: String, text: Binding<String>, secure: Bool, focus f: Field) -> some View {
        Group {
            if secure { SecureField(title, text: text) } else { TextField(title, text: text) }
        }
        .textFieldStyle(.plain)
        .font(.system(size: 15))
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .glassEffect(.regular, in: .rect(cornerRadius: 12))
        .focused($focus, equals: f)
        .onSubmit(submit)
    }

    private func submit() {
        guard !working else { return }
        working = true
        error = nil
        Task {
            let result: String?
            if needsCode {
                result = await model.notes.submitCode(code)
                if result != nil { code = "" }
            } else {
                result = await model.notes.signIn(appleID: appleID.trimmingCharacters(in: .whitespaces), password: password)
                if result == "code" { focus = .code }
            }
            if let result, result != "code" { error = result }
            if result == nil { password = "" }
            working = false
        }
    }
}
