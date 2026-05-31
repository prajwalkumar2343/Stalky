import Cocoa
import Combine
import SwiftUI

@available(macOS 14.0, *)
@MainActor
class AppDelegate: NSObject, NSApplicationDelegate {
    var statusItem: NSStatusItem?
    var overlayWindow: OverlayWindow?
    var quickEnhancePanel: QuickEnhancePanel?
    let service = AppService(config: AppConfig.loadDefault())
    private var isTerminating = false
    private var terminationReplySent = false
    private var cancellables = Set<AnyCancellable>()
    
    func applicationDidFinishLaunching(_ notification: Notification) {
        setupStatusItem()

        service.start()

        overlayWindow = OverlayWindow(
            position: service.config.app.overlayPosition,
            customOrigin: service.config.app.overlayOrigin,
            showsOverFullScreenApps: service.config.app.overlayShowsOverFullScreenApps,
            onOriginChanged: { [weak service] origin in
                service?.persistOverlayOrigin(origin)
            }
        )
        overlayWindow?.showPersistent()

        service.$config
            .sink { [weak self] config in
                self?.overlayWindow?.update(
                    position: config.app.overlayPosition,
                    customOrigin: config.app.overlayOrigin,
                    showsOverFullScreenApps: config.app.overlayShowsOverFullScreenApps
                )
                self?.overlayWindow?.showPersistent()
            }
            .store(in: &cancellables)

        service.$overlayActivity
            .removeDuplicates()
            .sink { [weak self] activity in
                self?.overlayWindow?.update(activity: activity)
            }
            .store(in: &cancellables)
    }
    
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !isTerminating else {
            return .terminateNow
        }

        isTerminating = true
        Task { @MainActor in
            await service.stop()
            replyToTerminationIfNeeded()
        }
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 5_000_000_000)
            replyToTerminationIfNeeded()
        }

        return .terminateLater
    }

    private func replyToTerminationIfNeeded() {
        guard !terminationReplySent else { return }
        terminationReplySent = true
        NSApp.reply(toApplicationShouldTerminate: true)
    }
    
    private func setupStatusItem() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        statusItem?.button?.image = NSImage(systemSymbolName: "brain", accessibilityDescription: "AuraBot")
        
        let menu = NSMenu()
        menu.addItem(NSMenuItem(title: "Show AuraBot", action: #selector(showMainWindow), keyEquivalent: ""))
        menu.addItem(NSMenuItem.separator())
        menu.addItem(NSMenuItem(title: "Quick Enhance", action: #selector(triggerQuickEnhance), keyEquivalent: ""))
        menu.addItem(NSMenuItem.separator())
        menu.addItem(NSMenuItem(title: "Quit", action: #selector(NSApp.terminate(_:)), keyEquivalent: "q"))
        
        statusItem?.menu = menu
    }
    
    @objc private func showMainWindow() {
        NSApp.activate(ignoringOtherApps: true)
        
        for window in NSApp.windows {
            window.makeKeyAndOrderFront(nil)
        }
    }
    
    @objc func triggerQuickEnhance() {
        // Get selected text
        let pasteboard = NSPasteboard.general
        let originalContent = pasteboard.string(forType: .string)
        
        // Simulate copy
        simulateCopy()
        
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in
            Task { @MainActor in
                let selectedText = pasteboard.string(forType: .string) ?? ""
                
                // Restore original
                if let original = originalContent {
                    pasteboard.clearContents()
                    pasteboard.setString(original, forType: .string)
                }
                
                self?.overlayWindow?.showPersistent()
                
                // Show quick enhance panel
                self?.showQuickEnhance(text: selectedText)
            }
        }
    }
    
    private func showQuickEnhance(text: String = "") {
        if quickEnhancePanel == nil {
            quickEnhancePanel = QuickEnhancePanel(service: service)
        }
        
        quickEnhancePanel?.setText(text)
        quickEnhancePanel?.show()
    }
    
    private func simulateCopy() {
        let source = CGEventSource(stateID: .combinedSessionState)
        
        let cmdDown = CGEvent(keyboardEventSource: source, virtualKey: 0x37, keyDown: true)
        let cDown = CGEvent(keyboardEventSource: source, virtualKey: 0x08, keyDown: true)
        let cUp = CGEvent(keyboardEventSource: source, virtualKey: 0x08, keyDown: false)
        let cmdUp = CGEvent(keyboardEventSource: source, virtualKey: 0x37, keyDown: false)
        
        cmdDown?.flags = .maskCommand
        cDown?.flags = .maskCommand
        
        cmdDown?.post(tap: .cghidEventTap)
        cDown?.post(tap: .cghidEventTap)
        cUp?.post(tap: .cghidEventTap)
        cmdUp?.post(tap: .cghidEventTap)
    }
}
