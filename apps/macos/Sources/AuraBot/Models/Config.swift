import Foundation

struct AppConfig: Codable {
    var capture: CaptureConfig
    var llm: LLMConfig
    var memory: MemoryConfig
    var app: AppSettings
    var browserExtension: ExtensionConfig
    var computerUse: ComputerUseConfig
    
    static let `default` = AppConfig(
        capture: CaptureConfig(),
        llm: LLMConfig(),
        memory: MemoryConfig(),
        app: AppSettings(),
        browserExtension: ExtensionConfig(),
        computerUse: ComputerUseConfig()
    )

    enum CodingKeys: String, CodingKey {
        case capture
        case llm
        case memory
        case app
        case browserExtension = "extension"
        case computerUse
    }

    init(
        capture: CaptureConfig = CaptureConfig(),
        llm: LLMConfig = LLMConfig(),
        memory: MemoryConfig = MemoryConfig(),
        app: AppSettings = AppSettings(),
        browserExtension: ExtensionConfig = ExtensionConfig(),
        computerUse: ComputerUseConfig = ComputerUseConfig()
    ) {
        self.capture = capture
        self.llm = llm
        self.memory = memory
        self.app = app
        self.browserExtension = browserExtension
        self.computerUse = computerUse
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        capture = try container.decodeIfPresent(CaptureConfig.self, forKey: .capture) ?? CaptureConfig()
        llm = try container.decodeIfPresent(LLMConfig.self, forKey: .llm) ?? LLMConfig()
        memory = try container.decodeIfPresent(MemoryConfig.self, forKey: .memory) ?? MemoryConfig()
        app = try container.decodeIfPresent(AppSettings.self, forKey: .app) ?? AppSettings()
        browserExtension = try container.decodeIfPresent(ExtensionConfig.self, forKey: .browserExtension) ?? ExtensionConfig()
        computerUse = try container.decodeIfPresent(ComputerUseConfig.self, forKey: .computerUse) ?? ComputerUseConfig()
    }
}

struct CaptureConfig: Codable {
    var intervalSeconds: Int = 30
    var quality: Int = 60
    var maxWidth: Int = 1280
    var maxHeight: Int = 720
    var enabled: Bool = true
    var probeIntervalSeconds: Int = 5
    var minCaptureGapSeconds: Int = 20
    var idleCaptureSeconds: Int = 300
    var previewWidth: Int = 160
    var previewHeight: Int = 90
    var meaningfulChangeThreshold: Int = 10
    var scrollCaptureCooldownSeconds: Int = 20
}

struct LLMConfig: Codable {
    var baseURL: String = "https://openrouter.ai/api/v1"
    var model: String = "google/gemini-flash-1.5"
    var maxTokens: Int = 512
    var temperature: Double = 0.7
    var timeoutSeconds: Int = 30
    var openRouterAPIKey: String = ""
    var openRouterChatModel: String = "anthropic/claude-3.5-sonnet"
    var contextCollectorRewrite: ContextCollectorRewritePolicy = .default

    func allowsContextCollectorRewrite(for modelIdentifier: String? = nil) -> Bool {
        contextCollectorRewrite.allows(modelIdentifier ?? openRouterChatModel)
    }
}

struct ContextCollectorRewritePolicy: Codable {
    var enabled: Bool = false
    var allowedModels: [ContextCollectorRewriteModelRule] = ContextCollectorRewriteModelRule.defaultRules

    static let `default` = ContextCollectorRewritePolicy()

    func allows(_ modelIdentifier: String) -> Bool {
        guard enabled else { return false }
        return allowedModels.contains { $0.matches(modelIdentifier) }
    }
}

struct ContextCollectorRewriteModelRule: Codable, Equatable, Sendable {
    let label: String
    let minimumVersion: Double
    let matchPatterns: [String]
    let requiredTokens: [String]

    init(
        label: String,
        minimumVersion: Double,
        matchPatterns: [String],
        requiredTokens: [String] = []
    ) {
        self.label = label
        self.minimumVersion = minimumVersion
        self.matchPatterns = matchPatterns
        self.requiredTokens = requiredTokens
    }

    func matches(_ modelIdentifier: String) -> Bool {
        let normalized = modelIdentifier.lowercased()
        guard requiredTokens.allSatisfy({ normalized.contains($0.lowercased()) }) else {
            return false
        }

        guard let version = extractedVersion(from: normalized) else {
            return false
        }

        return version >= minimumVersion
    }

    private func extractedVersion(from modelIdentifier: String) -> Double? {
        for pattern in matchPatterns {
            guard let regex = try? NSRegularExpression(pattern: pattern, options: [.caseInsensitive]) else {
                continue
            }

            let range = NSRange(modelIdentifier.startIndex..., in: modelIdentifier)
            guard let match = regex.firstMatch(in: modelIdentifier, options: [], range: range),
                  match.numberOfRanges > 1,
                  let versionRange = Range(match.range(at: 1), in: modelIdentifier) else {
                continue
            }

            return Double(modelIdentifier[versionRange])
        }

        return nil
    }

    static let defaultRules: [ContextCollectorRewriteModelRule] = [
        ContextCollectorRewriteModelRule(
            label: "Gemini >= 3.1",
            minimumVersion: 3.1,
            matchPatterns: ["gemini[-_ ]?(\\d+(?:\\.\\d+)?)"]
        ),
        ContextCollectorRewriteModelRule(
            label: "Claude Opus >= 4.5",
            minimumVersion: 4.5,
            matchPatterns: [
                "claude[-_ ]?opus[-_ ]?(\\d+(?:\\.\\d+)?)",
                "claude[-_ ]?(\\d+(?:\\.\\d+)?)[:/_ -]?opus"
            ],
            requiredTokens: ["claude", "opus"]
        ),
        ContextCollectorRewriteModelRule(
            label: "GPT >= 5.3",
            minimumVersion: 5.3,
            matchPatterns: ["gpt[-_ ]?(\\d+(?:\\.\\d+)?)"]
        ),
        ContextCollectorRewriteModelRule(
            label: "Kimi >= 2.5",
            minimumVersion: 2.5,
            matchPatterns: ["kimi[-_ ]?(\\d+(?:\\.\\d+)?)"]
        )
    ]
}

struct MemoryConfig: Codable {
    static let managedPgliteBaseURL = "http://127.0.0.1:8766"

    var apiKey: String = ""
    var baseURL: String = MemoryConfig.managedPgliteBaseURL
    var userID: String = "default_user"
    var collectionName: String = "screen_memories_v3"

    enum CodingKeys: String, CodingKey {
        case apiKey
        case baseURL
        case userID
        case collectionName
    }

    init() {}

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        apiKey = try container.decodeIfPresent(String.self, forKey: .apiKey) ?? ""
        baseURL = try container.decodeIfPresent(String.self, forKey: .baseURL) ?? Self.managedPgliteBaseURL
        userID = try container.decodeIfPresent(String.self, forKey: .userID) ?? "default_user"
        collectionName = try container.decodeIfPresent(String.self, forKey: .collectionName) ?? "screen_memories_v3"

        if Self.isLegacyDefaultMemoryURL(baseURL) {
            baseURL = Self.managedPgliteBaseURL
        }
    }

    private static func isLegacyDefaultMemoryURL(_ value: String) -> Bool {
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        return normalized == "http://localhost:8000" || normalized == "http://127.0.0.1:8000"
    }
}

struct AppSettings: Codable {
    var verbose: Bool = false
    var processOnCapture: Bool = true
    var memoryWindow: Int = 10
    var overlayPosition: OverlayPosition = .bottomRight
    var overlayOrigin: OverlayOrigin?
    var overlayShowsOverFullScreenApps: Bool = true
    var onboardingCompleted: Bool = false
    var activePluginID: String?

    enum CodingKeys: String, CodingKey {
        case verbose
        case processOnCapture
        case memoryWindow
        case overlayPosition
        case overlayOrigin
        case overlayShowsOverFullScreenApps
        case onboardingCompleted
        case activePluginID
    }

    init() {}

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        verbose = try container.decodeIfPresent(Bool.self, forKey: .verbose) ?? false
        processOnCapture = try container.decodeIfPresent(Bool.self, forKey: .processOnCapture) ?? true
        memoryWindow = try container.decodeIfPresent(Int.self, forKey: .memoryWindow) ?? 10
        overlayPosition = try container.decodeIfPresent(OverlayPosition.self, forKey: .overlayPosition) ?? .bottomRight
        overlayOrigin = try container.decodeIfPresent(OverlayOrigin.self, forKey: .overlayOrigin)
        overlayShowsOverFullScreenApps = try container.decodeIfPresent(Bool.self, forKey: .overlayShowsOverFullScreenApps) ?? true
        onboardingCompleted = try container.decodeIfPresent(Bool.self, forKey: .onboardingCompleted) ?? false
        activePluginID = try container.decodeIfPresent(String.self, forKey: .activePluginID)
    }
}

struct OverlayOrigin: Codable, Equatable, Sendable {
    var x: Double
    var y: Double
}

struct ComputerUseConfig: Codable, Equatable {
    var enabled: Bool = false
    var recordTrajectories: Bool = false
    var captureMode: ComputerUseCaptureMode = .som
    var maxImageDimension: Int = 1600

    enum CodingKeys: String, CodingKey {
        case enabled
        case recordTrajectories
        case captureMode
        case maxImageDimension
    }

    init() {}

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        enabled = try container.decodeIfPresent(Bool.self, forKey: .enabled) ?? false
        recordTrajectories = try container.decodeIfPresent(Bool.self, forKey: .recordTrajectories) ?? false
        captureMode = try container.decodeIfPresent(ComputerUseCaptureMode.self, forKey: .captureMode) ?? .som
        maxImageDimension = try container.decodeIfPresent(Int.self, forKey: .maxImageDimension) ?? 1600
    }
}

enum ComputerUseCaptureMode: String, Codable, CaseIterable, Identifiable, Sendable {
    case som
    case vision
    case ax

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .som:
            return "SOM"
        case .vision:
            return "Vision"
        case .ax:
            return "Accessibility"
        }
    }

    var description: String {
        switch self {
        case .som:
            return "Capture screenshots and accessibility structure."
        case .vision:
            return "Capture screenshots without indexing controls."
        case .ax:
            return "Read accessibility structure without screenshots."
        }
    }
}

enum OverlayPosition: String, Codable, CaseIterable, Identifiable, Sendable {
    case topRight = "top_right"
    case bottomRight = "bottom_right"

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .topRight:
            return "Top Right"
        case .bottomRight:
            return "Bottom Right"
        }
    }
}

struct ExtensionConfig: Codable {
    var enabled: Bool = true
    var port: Int = 7345
    var freshnessSeconds: Int = 15
    var apiKey: String = ""
    var allowedOrigins: [String] = [
        "chrome-extension://",
        "moz-extension://",
        "safari-web-extension://",
        "http://localhost:",
        "http://127.0.0.1:"
    ]

    enum CodingKeys: String, CodingKey {
        case enabled
        case port
        case freshnessSeconds
        case apiKey
        case allowedOrigins
    }

    init() {}

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        enabled = try container.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        port = try container.decodeIfPresent(Int.self, forKey: .port) ?? 7345
        freshnessSeconds = try container.decodeIfPresent(Int.self, forKey: .freshnessSeconds) ?? 15
        apiKey = try container.decodeIfPresent(String.self, forKey: .apiKey) ?? ""
        allowedOrigins = try container.decodeIfPresent([String].self, forKey: .allowedOrigins) ?? [
            "chrome-extension://",
            "moz-extension://",
            "safari-web-extension://",
            "http://localhost:",
            "http://127.0.0.1:"
        ]
    }
}

extension AppConfig {
    static var defaultURL: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".aurabot", isDirectory: true)
            .appendingPathComponent("config.json")
    }

    static func loadDefault() -> AppConfig {
        load(from: defaultURL.path)
    }

    static func load(from path: String) -> AppConfig {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
              let config = try? JSONDecoder().decode(AppConfig.self, from: data) else {
            return .default
        }
        return config
    }
    
    func save(to path: String) throws {
        let url = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = try encoder.encode(self)
        try data.write(to: url, options: [.atomic])
    }
}
