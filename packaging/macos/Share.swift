import AppKit

// An AppKit process keeps the native service alive after Folio hands off a PDF.
// Arguments are passed directly, never interpolated into a script or shell.
@MainActor
final class SharingApp: NSObject, NSApplicationDelegate, NSWindowDelegate,
    @preconcurrency NSSharingServicePickerDelegate, NSSharingServiceDelegate {
    private var window: NSWindow!
    private var picker: NSSharingServicePicker?
    private var service: NSSharingService?
    private var ready = false
    private let mode: String
    private let file: URL
    private let subject: String

    init(mode: String, file: URL, subject: String) {
        self.mode = mode
        self.file = file
        self.subject = subject
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard FileManager.default.isReadableFile(atPath: file.path) else {
            fail("The retained PDF could not be read.")
            return
        }
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 420, height: 130),
                          styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = mode == "email" ? "Email PDF" : "Share PDF"
        window.delegate = self
        window.isReleasedWhenClosed = false
        let label = NSTextField(labelWithString: file.lastPathComponent)
        label.lineBreakMode = .byTruncatingMiddle
        label.frame = NSRect(x: 24, y: 78, width: 372, height: 22)
        window.contentView!.addSubview(label)
        let button = NSButton(title: mode == "email" ? "Open Email Composer" : "Share…",
                              target: self, action: #selector(share))
        button.bezelStyle = .rounded
        button.frame = NSRect(x: 225, y: 24, width: 172, height: 32)
        window.contentView!.addSubview(button)
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        DispatchQueue.main.async { self.share(button) }
    }

    @objc private func share(_ button: NSButton) {
        if mode == "email" {
            guard let service = NSSharingService(named: .composeEmail),
                  service.canPerform(withItems: [file]) else {
                fail("No email sharing service is available. Configure a mail application first.")
                return
            }
            self.service = service
            service.delegate = self
            service.subject = subject
            service.perform(withItems: [file])
            window.orderOut(nil)
        } else {
            let picker = NSSharingServicePicker(items: [file])
            self.picker = picker
            picker.delegate = self
            picker.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        }
        if !ready {
            ready = true
            FileHandle.standardOutput.write(Data("READY\n".utf8))
        }
    }

    func sharingServicePicker(_ sharingServicePicker: NSSharingServicePicker,
                              didChoose service: NSSharingService?) {
        guard let service = service else { NSApp.terminate(nil); return }
        self.service = service
        service.subject = subject
        window.orderOut(nil)
    }

    func sharingServicePicker(_ sharingServicePicker: NSSharingServicePicker,
                              delegateFor sharingService: NSSharingService) -> NSSharingServiceDelegate? {
        self.service = sharingService
        sharingService.subject = subject
        return self
    }

    func sharingService(_ sharingService: NSSharingService, didShareItems items: [Any]) {
        NSApp.terminate(nil)
    }

    func sharingService(_ sharingService: NSSharingService, didFailToShareItems items: [Any], error: Error) {
        // A cancelled composer is a normal outcome.
        if (error as NSError).code == NSUserCancelledError {
            NSApp.terminate(nil)
        } else {
            fail(error.localizedDescription)
        }
    }

    func windowWillClose(_ notification: Notification) { NSApp.terminate(nil) }

    private func fail(_ message: String) {
        if ready {
            NSApp.activate(ignoringOtherApps: true)
            let alert = NSAlert()
            alert.messageText = "Could not share PDF"
            alert.informativeText = message
            alert.runModal()
        } else {
            FileHandle.standardOutput.write(Data("ERROR\t\(message.replacingOccurrences(of: "\n", with: " "))\n".utf8))
        }
        exit(1)
    }
}

@main
enum SharingMain {
    @MainActor
    static func main() {
        let arguments = CommandLine.arguments
        if arguments.count == 2 && arguments[1] == "--check" {
            // Check the packaged executable without opening a native window.
            print("Folio native sharing helper")
            return
        }
        guard arguments.count == 4, ["email", "share"].contains(arguments[1]) else {
            FileHandle.standardError.write(Data("Usage: folio-share email|share PDF SUBJECT\n".utf8))
            exit(2)
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let delegate = SharingApp(mode: arguments[1], file: URL(fileURLWithPath: arguments[2]), subject: arguments[3])
        app.delegate = delegate
        // NSApplication holds its delegate weakly. Retain it for the run loop.
        withExtendedLifetime(delegate) {
            app.run()
        }
    }
}
