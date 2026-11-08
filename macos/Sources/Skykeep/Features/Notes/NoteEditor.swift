import AppKit
import SwiftUI

/// Apple's paragraph styles, as Notes numbers them.
enum NoteStyle {
    static let title = 0, heading = 1, subheading = 2, body = 3, mono = 4
    static let dotted = 100, dashed = 101, numbered = 102, checklist = 103
    static let lists: Set<Int> = [dotted, dashed, numbered, checklist]
}

extension NSAttributedString.Key {
    static let noteStyle = NSAttributedString.Key("skykeep.note.style")
    static let noteChecked = NSAttributedString.Key("skykeep.note.checked")
    static let noteIndent = NSAttributedString.Key("skykeep.note.indent")
    static let noteQuote = NSAttributedString.Key("skykeep.note.quote")
}

/// A note's text and Apple's formatting runs, laid out as Notes lays them out. The string is the
/// note's own text, character for character — list markers and checkboxes are drawn beside it,
/// never typed into it — so what is edited here is exactly what is sent back to iCloud.
enum NoteFormatter {
    static let bodySize: CGFloat = 14

    static func font(for style: Int?) -> NSFont {
        switch style {
        case NoteStyle.title: .systemFont(ofSize: 24, weight: .bold)
        case NoteStyle.heading: .systemFont(ofSize: 19, weight: .bold)
        case NoteStyle.subheading: .systemFont(ofSize: 16, weight: .semibold)
        case NoteStyle.mono: .monospacedSystemFont(ofSize: 13, weight: .regular)
        default: .systemFont(ofSize: bodySize)
        }
    }

    static func paragraph(style: Int?, indent: Int, quote: Bool) -> NSParagraphStyle {
        let p = NSMutableParagraphStyle()
        p.lineSpacing = 3
        p.paragraphSpacing = style == NoteStyle.title ? 8 : (style == NoteStyle.heading || style == NoteStyle.subheading ? 4 : 2)
        var left = CGFloat(indent) * 22
        if let style, NoteStyle.lists.contains(style) { left += 28 }
        if quote { left += 16 }
        p.firstLineHeadIndent = left
        p.headIndent = left
        return p
    }

    static let emphasis: [Int: NSColor] = [1: .systemPurple, 2: .systemPink, 3: .systemOrange, 4: .systemMint, 5: .systemBlue]

    static var baseAttributes: [NSAttributedString.Key: Any] {
        [.font: font(for: nil), .foregroundColor: NSColor.labelColor, .paragraphStyle: paragraph(style: nil, indent: 0, quote: false)]
    }

    static func attributed(body: String, runs: [NoteRun]) -> NSAttributedString {
        let out = NSMutableAttributedString(string: body, attributes: baseAttributes)
        let length = (body as NSString).length
        var loc = 0
        for run in runs {
            let len = min(run.length, max(0, length - loc))
            guard len > 0 else { loc += run.length; continue }
            let r = NSRange(location: loc, length: len)
            var font = font(for: run.style)
            let fm = NSFontManager.shared
            if run.bold { font = fm.convert(font, toHaveTrait: .boldFontMask) }
            if run.italic { font = fm.convert(font, toHaveTrait: .italicFontMask) }
            var attrs: [NSAttributedString.Key: Any] = [
                .font: font,
                .paragraphStyle: paragraph(style: run.style, indent: run.indent, quote: run.quote),
                .noteIndent: run.indent,
            ]
            if let style = run.style { attrs[.noteStyle] = style }
            if run.quote { attrs[.noteQuote] = true }
            if run.style == NoteStyle.checklist { attrs[.noteChecked] = run.checked ?? false }
            if run.underline { attrs[.underlineStyle] = NSUnderlineStyle.single.rawValue }
            if run.strike { attrs[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
            if let e = run.emphasis, let c = emphasis[e] { attrs[.foregroundColor] = c }
            if run.style == NoteStyle.checklist, run.checked == true { attrs[.foregroundColor] = NSColor.secondaryLabelColor }
            if let link = run.link, let url = URL(string: link) { attrs[.link] = url }
            out.addAttributes(attrs, range: r)
            if let id = run.attachment, len == 1 {
                out.addAttribute(.attachment, value: attachment(id: id), range: r)
            }
            loc += run.length
        }
        // Notes shows the first line as the title until it is given another style.
        if length > 0, out.attribute(.noteStyle, at: 0, effectiveRange: nil) == nil {
            let first = (body as NSString).paragraphRange(for: NSRange(location: 0, length: 0))
            out.addAttributes([.font: font(for: NoteStyle.title), .paragraphStyle: paragraph(style: NoteStyle.title, indent: 0, quote: false)], range: first)
        }
        return out
    }

    /// A picture where Notes on this Mac keeps one; otherwise a quiet labelled tile, so the
    /// note's layout (and its text, character for character) stays as it is.
    static func attachment(id: String) -> NSTextAttachment {
        let att = NSTextAttachment()
        if let img = NotesService.localImage(for: id) {
            let maxW: CGFloat = 480
            let scale = min(1, maxW / max(img.size.width, 1))
            att.image = img
            att.bounds = CGRect(x: 0, y: 0, width: img.size.width * scale, height: img.size.height * scale)
        } else {
            let size = NSSize(width: 150, height: 34)
            att.image = NSImage(size: size, flipped: false) { rect in
                NSColor.quaternaryLabelColor.setFill()
                NSBezierPath(roundedRect: rect.insetBy(dx: 0.5, dy: 0.5), xRadius: 8, yRadius: 8).fill()
                let label = NSAttributedString(string: "📎 Anexo", attributes: [.font: NSFont.systemFont(ofSize: 12, weight: .medium), .foregroundColor: NSColor.secondaryLabelColor])
                label.draw(at: NSPoint(x: 12, y: (rect.height - label.size().height) / 2))
                return true
            }
            att.bounds = CGRect(origin: .zero, size: size)
        }
        return att
    }
}

/// The note's page: text edited in place, with list markers, numbers and Notes' round
/// checkboxes drawn in the margin. A click on a checkbox ticks it.
final class NoteTextView: NSTextView {
    var onToggle: ((_ start: Int, _ checked: Bool, _ text: String) -> Void)?

    private struct Marker { let rect: NSRect; let style: Int; let start: Int; let checked: Bool; let number: Int; let text: String }

    private func markers(in dirty: NSRect? = nil) -> [Marker] {
        guard let lm = layoutManager, let tc = textContainer, let storage = textStorage, storage.length > 0 else { return [] }
        let s = storage.string as NSString
        var out: [Marker] = []
        var number = 0
        var p = 0
        let origin = textContainerOrigin
        while p < s.length {
            let para = s.paragraphRange(for: NSRange(location: p, length: 0))
            let style = storage.attribute(.noteStyle, at: para.location, effectiveRange: nil) as? Int
            number = style == NoteStyle.numbered ? number + 1 : 0
            if let style, NoteStyle.lists.contains(style) {
                let glyph = lm.glyphIndexForCharacter(at: para.location)
                if glyph < lm.numberOfGlyphs {
                    let line = lm.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil)
                    let ps = storage.attribute(.paragraphStyle, at: para.location, effectiveRange: nil) as? NSParagraphStyle
                    let indent = ps?.firstLineHeadIndent ?? 28
                    let size: CGFloat = 17
                    let used = lm.lineFragmentUsedRect(forGlyphAt: glyph, effectiveRange: nil)
                    let h = max(used.height, line.height)
                    let rect = NSRect(x: origin.x + tc.lineFragmentPadding + indent - 26, y: origin.y + line.minY + (h - size) / 2 - 1, width: size, height: size)
                    var text = s.substring(with: para)
                    while text.hasSuffix("\n") || text.hasSuffix("\u{2028}") { text.removeLast() }
                    let checked = (storage.attribute(.noteChecked, at: para.location, effectiveRange: nil) as? Bool) ?? false
                    if dirty == nil || rect.intersects(dirty!) {
                        out.append(Marker(rect: rect, style: style, start: para.location, checked: checked, number: number, text: text))
                    }
                }
            }
            if let q = storage.attribute(.noteQuote, at: para.location, effectiveRange: nil) as? Bool, q {
                // A quote's bar is drawn with the markers below.
            }
            p = para.location + max(para.length, 1)
        }
        return out
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        for m in markers(in: dirtyRect) {
            switch m.style {
            case NoteStyle.checklist:
                let circle = NSBezierPath(ovalIn: m.rect.insetBy(dx: 0.75, dy: 0.75))
                if m.checked {
                    NSColor.systemOrange.setFill()
                    circle.fill()
                    let check = NSBezierPath()
                    let r = m.rect
                    check.move(to: NSPoint(x: r.minX + r.width * 0.28, y: r.minY + r.height * 0.52))
                    check.line(to: NSPoint(x: r.minX + r.width * 0.44, y: r.minY + r.height * 0.68))
                    check.line(to: NSPoint(x: r.minX + r.width * 0.73, y: r.minY + r.height * 0.34))
                    check.lineWidth = 1.8
                    check.lineCapStyle = .round
                    check.lineJoinStyle = .round
                    NSColor.white.setStroke()
                    check.stroke()
                } else {
                    NSColor.tertiaryLabelColor.setStroke()
                    circle.lineWidth = 1.3
                    circle.stroke()
                }
            default:
                let glyph: String
                switch m.style {
                case NoteStyle.dotted: glyph = "•"
                case NoteStyle.dashed: glyph = "–"
                default: glyph = "\(m.number)."
                }
                let a = NSAttributedString(string: glyph, attributes: [.font: NSFont.systemFont(ofSize: NoteFormatter.bodySize), .foregroundColor: NSColor.secondaryLabelColor])
                let size = a.size()
                a.draw(at: NSPoint(x: m.rect.maxX - size.width, y: m.rect.midY - size.height / 2))
            }
        }
    }

    override var isFlipped: Bool { true }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if let hit = markers().first(where: { $0.style == NoteStyle.checklist && $0.rect.insetBy(dx: -6, dy: -4).contains(point) }) {
            setChecked(!hit.checked, paragraphAt: hit.start)
            onToggle?(hit.start, !hit.checked, hit.text)
            return
        }
        super.mouseDown(with: event)
    }

    override func resetCursorRects() {
        super.resetCursorRects()
        for m in markers() where m.style == NoteStyle.checklist {
            addCursorRect(m.rect.insetBy(dx: -6, dy: -4), cursor: .pointingHand)
        }
    }

    /// Shown at once; iCloud is told behind.
    func setChecked(_ checked: Bool, paragraphAt start: Int) {
        guard let storage = textStorage else { return }
        let para = (storage.string as NSString).paragraphRange(for: NSRange(location: start, length: 0))
        storage.beginEditing()
        storage.addAttribute(.noteChecked, value: checked, range: para)
        storage.addAttribute(.foregroundColor, value: checked ? NSColor.secondaryLabelColor : NSColor.labelColor, range: para)
        storage.endEditing()
        needsDisplay = true
    }

    /// The paragraph the caret is in: where it starts and its text.
    var currentParagraph: (start: Int, text: String)? {
        guard let storage = textStorage else { return nil }
        let s = storage.string as NSString
        let at = min(selectedRange().location, max(0, s.length - 1))
        guard s.length > 0 else { return nil }
        let para = s.paragraphRange(for: NSRange(location: at, length: 0))
        var text = s.substring(with: para)
        while text.hasSuffix("\n") { text.removeLast() }
        return (para.location, text)
    }

    override func didChangeText() {
        super.didChangeText()
        needsDisplay = true
    }
}

/// Lets the note's toolbar reach the text view (the caret's paragraph, a pending save).
@MainActor
final class NoteEditorHandle {
    weak var textView: NoteTextView?
}

struct NoteEditor: NSViewRepresentable {
    let text: NSAttributedString
    /// Changes when the note is read again from iCloud; the text is replaced only then.
    let version: Int
    let editable: Bool
    let handle: NoteEditorHandle
    let onChange: (String) -> Void
    let onToggle: (Int, Bool, String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSScrollView {
        let storage = NSTextStorage()
        let layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        let container = NSTextContainer(containerSize: NSSize(width: 0, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = true
        layout.addTextContainer(container)
        let tv = NoteTextView(frame: .zero, textContainer: container)
        tv.isVerticallyResizable = true
        tv.isHorizontallyResizable = false
        tv.autoresizingMask = [.width]
        tv.minSize = NSSize(width: 0, height: 0)
        tv.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        tv.drawsBackground = false
        tv.isRichText = true
        tv.allowsUndo = true
        tv.isAutomaticQuoteSubstitutionEnabled = false
        tv.isAutomaticDashSubstitutionEnabled = false
        tv.textContainerInset = NSSize(width: 40, height: 18)
        tv.typingAttributes = NoteFormatter.baseAttributes
        tv.delegate = context.coordinator
        tv.onToggle = { start, checked, text in context.coordinator.parent.onToggle(start, checked, text) }
        storage.setAttributedString(text)
        tv.isEditable = editable
        handle.textView = tv
        context.coordinator.version = version

        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.documentView = tv
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.parent = self
        guard let tv = scroll.documentView as? NoteTextView else { return }
        tv.isEditable = editable
        handle.textView = tv
        if context.coordinator.version != version {
            context.coordinator.version = version
            let selection = tv.selectedRange()
            context.coordinator.applying = true
            tv.textStorage?.setAttributedString(text)
            context.coordinator.applying = false
            let len = (tv.string as NSString).length
            tv.setSelectedRange(NSRange(location: min(selection.location, len), length: 0))
            tv.needsDisplay = true
            tv.window?.invalidateCursorRects(for: tv)
        }
    }

    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: NoteEditor
        var version = 0
        var applying = false
        init(_ p: NoteEditor) { parent = p }

        func textDidChange(_ notification: Notification) {
            guard !applying, let tv = notification.object as? NSTextView else { return }
            parent.onChange(tv.string)
        }
    }
}
