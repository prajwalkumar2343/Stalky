import Foundation

enum ContextMode: String, Sendable {
    case browserResearch = "browser_research"
    case codingIDE = "coding_ide"
    case terminalDebugging = "terminal_debugging"
    case documentWriting = "document_writing"
    case meetingOrCall = "meeting_or_call"
    case genericVisual = "generic_visual"
    case idleOrDuplicate = "idle_or_duplicate"
}

enum ScreenshotDirective: Sendable {
    case skip
    case fallback
}

struct ActiveAppSnapshot: Sendable {
    let name: String
    let bundleIdentifier: String?
    let processIdentifier: Int32
    let windowTitle: String?
    let timestamp: Date

    var displayName: String {
        if let windowTitle, !windowTitle.isEmpty {
            return "\(name) - \(windowTitle)"
        }
        return name
    }
}

struct GitContext: Sendable {
    let rootPath: String
    let branch: String?
    let dirtyFiles: [String]

    var projectName: String {
        URL(fileURLWithPath: rootPath).lastPathComponent
    }

    var fingerprint: String {
        "\(rootPath)|\(branch ?? "")|\(dirtyFiles.joined(separator: ","))"
    }

    var summary: String {
        var parts = ["Project: \(projectName)"]
        if let branch, !branch.isEmpty {
            parts.append("Branch: \(branch)")
        }
        if dirtyFiles.isEmpty {
            parts.append("Working tree clean")
        } else {
            parts.append("Dirty files: \(dirtyFiles.prefix(8).joined(separator: ", "))")
        }
        return parts.joined(separator: " | ")
    }
}

struct ContextEvent: Sendable {
    let mode: ContextMode
    let source: String
    let summary: String
    let activities: [String]
    let keyElements: [String]
    let userIntent: String
    let importance: Double
    let ttl: String
    let fingerprint: String
    let timestamp: Date
    let activeApp: ActiveAppSnapshot?
    let gitContext: GitContext?
    let filePath: String?
    let browserContext: BrowserContext?
    let captureReason: String

    var memoryContent: String {
        var parts = [
            "Mode: \(mode.rawValue)",
            "Source: \(source)",
            "Summary: \(summary)",
            "Intent: \(userIntent)",
            "TTL: \(ttl)",
            String(format: "Importance: %.2f", importance)
        ]

        if !keyElements.isEmpty {
            parts.append("Key elements: \(keyElements.joined(separator: ", "))")
        }

        return parts.joined(separator: " | ")
    }

    func metadata(displayNum: Int = 0) -> Metadata {
        Metadata(
            timestamp: ISO8601DateFormatter().string(from: timestamp),
            context: mode.rawValue,
            activities: activities,
            keyElements: keyElements,
            userIntent: userIntent,
            displayNum: displayNum,
            browser: browserContext?.browser,
            url: browserContext?.url,
            captureReason: captureReason,
            visibleTextHash: browserContext?.visibleTextHash,
            readableTextHash: browserContext?.readableTextHash,
            textCaptureMode: browserContext?.textCaptureMode,
            pageTextSummary: browserContext?.readableTextSummary ?? browserContext?.visibleTextExcerpt,
            browserSourceQuality: browserContext?.sourceQuality.rawValue,
            browserCaptureID: browserContext?.captureID,
            browserSchemaVersion: browserContext?.schemaVersion,
            appName: activeApp?.name ?? browserContext?.browser,
            windowTitle: activeApp?.windowTitle ?? browserContext?.title,
            bundleIdentifier: activeApp?.bundleIdentifier ?? browserContext?.bundleIdentifier,
            repoPath: gitContext?.rootPath,
            filePath: filePath
        )
    }
}

struct ContextCapturePlan: Sendable {
    let mode: ContextMode
    let confidence: Double
    let screenshotDirective: ScreenshotDirective
    let event: ContextEvent?
    let browserContext: BrowserContext?
    let reason: String
}
