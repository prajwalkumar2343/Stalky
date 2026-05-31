import AppKit
import Foundation

enum BrowserContextUnavailableReason: String, Equatable, Sendable {
    case noActiveBrowser = "no_active_browser"
    case extensionStale = "extension_stale"
    case automationUnavailable = "automation_unavailable"
}

struct BrowserContextStatus: Sendable {
    let context: BrowserContext?
    let staleExtensionContext: BrowserContext?
    let reason: BrowserContextUnavailableReason?

    var hasFreshExtensionContext: Bool {
        context?.source == .extensionData
    }
}

actor BrowserContextService {
    static let maxContextFutureSkewSeconds: TimeInterval = 60

    private let config: ExtensionConfig
    private var latestExtensionContext: BrowserContext?

    init(config: ExtensionConfig) {
        self.config = config
    }

    func updateExtensionContext(_ context: BrowserContext) {
        latestExtensionContext = context
    }

    func currentContext() async -> BrowserContext? {
        await currentContextStatus().context
    }

    func currentContextStatus() async -> BrowserContextStatus {
        let now = Date()
        if let latestExtensionContext,
           Self.isContextFresh(
            timestamp: latestExtensionContext.timestamp,
            now: now,
            freshnessSeconds: config.freshnessSeconds
           ) {
            return BrowserContextStatus(
                context: latestExtensionContext,
                staleExtensionContext: nil,
                reason: nil
            )
        }

        let automation = automationContext()
        let staleReason: BrowserContextUnavailableReason? = latestExtensionContext == nil ? nil : .extensionStale

        if let automation {
            return BrowserContextStatus(
                context: automation,
                staleExtensionContext: latestExtensionContext,
                reason: staleReason
            )
        }

        return BrowserContextStatus(
            context: nil,
            staleExtensionContext: latestExtensionContext,
            reason: latestExtensionContext == nil ? .noActiveBrowser : .extensionStale
        )
    }

    private func automationContext() -> BrowserContext? {
        guard let app = NSWorkspace.shared.frontmostApplication,
              let bundleIdentifier = app.bundleIdentifier,
              let browser = supportedBrowser(for: bundleIdentifier) else {
            return nil
        }

        let snapshot = browserSnapshot(for: browser)
        let normalizedURL = Self.normalizedOptionalString(snapshot.url)
        let normalizedTitle = Self.normalizedOptionalString(snapshot.title)
        let derived = BrowserContextService.deriveActivity(url: normalizedURL, title: normalizedTitle)

        return BrowserContext(
            source: .automation,
            browser: browser.displayName,
            bundleIdentifier: bundleIdentifier,
            url: normalizedURL,
            title: normalizedTitle,
            activity: derived.activity,
            pageID: derived.pageID,
            mediaID: derived.mediaID,
            mediaIsPlaying: derived.activity == .media,
            scrollPercent: nil,
            viewportSignature: nil,
            noveltyScore: nil,
            visibleText: nil,
            selectedText: nil,
            readableText: nil,
            visibleTextHash: nil,
            readableTextHash: nil,
            textCaptureMode: nil,
            privateWindow: false,
            sourceQuality: .automationFallback,
            timestamp: Date()
        )
    }

    private func browserSnapshot(for browser: SupportedBrowser) -> (url: String?, title: String?) {
        let script = browser.appleScript
        guard !script.isEmpty else {
            return (nil, nil)
        }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        process.arguments = ["-e", script]

        let outputPipe = Pipe()
        let errorPipe = Pipe()
        process.standardOutput = outputPipe
        process.standardError = errorPipe

        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            return (nil, nil)
        }

        guard process.terminationStatus == 0 else {
            return (nil, nil)
        }

        let data = outputPipe.fileHandleForReading.readDataToEndOfFile()
        guard let output = String(data: data, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines),
            !output.isEmpty else {
            return (nil, nil)
        }

        let components = output.components(separatedBy: "\n")
        let url = components.first.flatMap { $0.isEmpty ? nil : $0 }
        let title = components.dropFirst().joined(separator: "\n")
        return (url, title.isEmpty ? nil : title)
    }

    private func supportedBrowser(for bundleIdentifier: String) -> SupportedBrowser? {
        SupportedBrowser.allCases.first(where: { $0.bundleIdentifier == bundleIdentifier })
    }

    static func deriveActivity(url: String?, title: String?) -> (activity: BrowserActivityKind, pageID: String?, mediaID: String?) {
        guard let rawURL = normalizedOptionalString(url), let components = URLComponents(string: rawURL) else {
            let fallbackPageID = normalizedOptionalString(title)
            return (.browsing, fallbackPageID, nil)
        }

        let host = components.host?.lowercased() ?? ""
        let path = components.path.lowercased()
        let queryItems = components.queryItems ?? []

        if host.contains("youtube.com") || host == "youtu.be" {
            let mediaID =
                queryItems.first(where: { $0.name == "v" })?.value ??
                path.split(separator: "/").last.map(String.init)

            if path == "/watch" || path.hasPrefix("/shorts") || host == "youtu.be" {
                return (.media, normalizedPageID(for: components), mediaID)
            }
        }

        let pageID = normalizedPageID(for: components)
        return (.browsing, pageID, nil)
    }

    static func normalizedPageID(for components: URLComponents) -> String {
        let host = components.host?.lowercased() ?? "unknown"
        let path = components.path.isEmpty ? "/" : components.path
        return host + path
    }

    static func normalizedOptionalString(_ value: String?) -> String? {
        guard let value else { return nil }
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return normalized.isEmpty ? nil : normalized
    }

    static func normalizedContextTimestamp(
        _ timestamp: Date?,
        now: Date = Date(),
        maxFutureSkewSeconds: TimeInterval = maxContextFutureSkewSeconds
    ) -> Date {
        guard let timestamp else {
            return now
        }

        if timestamp.timeIntervalSince(now) > maxFutureSkewSeconds {
            return now
        }

        return timestamp
    }

    static func isContextFresh(
        timestamp: Date,
        now: Date,
        freshnessSeconds: Int,
        maxFutureSkewSeconds: TimeInterval = maxContextFutureSkewSeconds
    ) -> Bool {
        let age = now.timeIntervalSince(timestamp)
        guard age >= -maxFutureSkewSeconds else {
            return false
        }

        return age <= TimeInterval(max(freshnessSeconds, 0))
    }
}

private enum SupportedBrowser: CaseIterable {
    case chrome
    case edge
    case brave
    case arc
    case safari

    var bundleIdentifier: String {
        switch self {
        case .chrome:
            return "com.google.Chrome"
        case .edge:
            return "com.microsoft.edgemac"
        case .brave:
            return "com.brave.Browser"
        case .arc:
            return "company.thebrowser.Browser"
        case .safari:
            return "com.apple.Safari"
        }
    }

    var displayName: String {
        switch self {
        case .chrome:
            return "Google Chrome"
        case .edge:
            return "Microsoft Edge"
        case .brave:
            return "Brave Browser"
        case .arc:
            return "Arc"
        case .safari:
            return "Safari"
        }
    }

    var appleScript: String {
        switch self {
        case .chrome:
            return """
            tell application "Google Chrome"
                if not (exists front window) then return ""
                set tabURL to URL of active tab of front window
                set tabTitle to title of active tab of front window
                return tabURL & linefeed & tabTitle
            end tell
            """
        case .edge:
            return """
            tell application "Microsoft Edge"
                if not (exists front window) then return ""
                set tabURL to URL of active tab of front window
                set tabTitle to title of active tab of front window
                return tabURL & linefeed & tabTitle
            end tell
            """
        case .brave:
            return """
            tell application "Brave Browser"
                if not (exists front window) then return ""
                set tabURL to URL of active tab of front window
                set tabTitle to title of active tab of front window
                return tabURL & linefeed & tabTitle
            end tell
            """
        case .arc:
            return """
            tell application "Arc"
                if not (exists front window) then return ""
                set tabURL to URL of active tab of front window
                set tabTitle to title of active tab of front window
                return tabURL & linefeed & tabTitle
            end tell
            """
        case .safari:
            return """
            tell application "Safari"
                if not (exists front document) then return ""
                set tabURL to URL of front document
                set tabTitle to name of front document
                return tabURL & linefeed & tabTitle
            end tell
            """
        }
    }
}
