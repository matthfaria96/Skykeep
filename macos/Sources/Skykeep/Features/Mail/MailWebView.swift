import SwiftUI
import WebKit

/// A message's HTML, shown the safe way: no scripts, nothing fetched from the internet
/// (tracking pixels and remote images stay blocked, as Mail Privacy Protection would), no
/// cookies kept. Links open only when clicked.
struct MailWebView: NSViewRepresentable {
    let html: String

    @MainActor private static var rules: WKContentRuleList?
    @MainActor private static var compiling = false

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        if let rules = Self.rules { config.userContentController.add(rules) }
        let view = WKWebView(frame: .zero, configuration: config)
        view.navigationDelegate = context.coordinator
        view.setValue(false, forKey: "drawsBackground")
        view.allowsMagnification = true
        if Self.rules == nil { Self.compileRules(then: view, html: html) } else { view.loadHTMLString(html, baseURL: nil) }
        context.coordinator.loaded = html
        return view
    }

    func updateNSView(_ view: WKWebView, context: Context) {
        guard context.coordinator.loaded != html else { return }
        context.coordinator.loaded = html
        view.loadHTMLString(html, baseURL: nil)
    }

    @MainActor private static func compileRules(then view: WKWebView, html: String) {
        let json = #"[{"trigger":{"url-filter":"^(https?|ftp|wss?)://"},"action":{"type":"block"}}]"#
        WKContentRuleListStore.default().compileContentRuleList(forIdentifier: "skykeep.mail.block-remote", encodedContentRuleList: json) { list, _ in
            Task { @MainActor in
                if let list {
                    MailWebView.rules = list
                    view.configuration.userContentController.add(list)
                }
                view.loadHTMLString(html, baseURL: nil)
            }
        }
    }

    final class Coordinator: NSObject, WKNavigationDelegate {
        var loaded = ""

        func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping @MainActor (WKNavigationActionPolicy) -> Void) {
            if action.navigationType == .linkActivated, let url = action.request.url {
                NSWorkspace.shared.open(url)
                decisionHandler(.cancel)
                return
            }
            let scheme = action.request.url?.scheme ?? ""
            decisionHandler(scheme == "about" || scheme == "data" || action.navigationType == .other ? .allow : .cancel)
        }
    }
}
