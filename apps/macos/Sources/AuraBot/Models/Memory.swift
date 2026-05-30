import Foundation

enum MemoryV2JSON {
    static let schemaVersion = "memory-v2"

    static func makeDecoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let container = try decoder.singleValueContainer()
            let value = try container.decode(String.self)

            guard let date = parseDate(value) else {
                throw DecodingError.dataCorruptedError(
                    in: container,
                    debugDescription: "Invalid ISO 8601 date: \(value)"
                )
            }

            return date
        }
        return decoder
    }

    static func makeEncoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .custom { date, encoder in
            var container = encoder.singleValueContainer()
            try container.encode(iso8601Formatter().string(from: date))
        }
        return encoder
    }

    static func parseDate(_ value: String) -> Date? {
        if let date = iso8601FractionalFormatter().date(from: value) {
            return date
        }
        if let date = iso8601Formatter().date(from: value) {
            return date
        }
        if let date = pythonDateFormatter().date(from: value) {
            return date
        }
        return pythonDateFormatterNoFraction().date(from: value)
    }

    private static func iso8601FractionalFormatter() -> ISO8601DateFormatter {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }

    private static func iso8601Formatter() -> ISO8601DateFormatter {
        ISO8601DateFormatter()
    }

    private static func pythonDateFormatter() -> DateFormatter {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        formatter.dateFormat = "yyyy-MM-dd'T'HH:mm:ss.SSSSSS"
        return formatter
    }

    private static func pythonDateFormatterNoFraction() -> DateFormatter {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        formatter.dateFormat = "yyyy-MM-dd'T'HH:mm:ss"
        return formatter
    }
}

enum MemorySource: Codable, Hashable, Sendable {
    case recentContext
    case recentSummary
    case brainPage
    case brainChunk
    case graph
    case timelineEvent
    case promotionCandidate
    case unknown(String)

    var rawValue: String {
        switch self {
        case .recentContext: return "recent_context"
        case .recentSummary: return "recent_summary"
        case .brainPage: return "brain_page"
        case .brainChunk: return "brain_chunk"
        case .graph: return "graph"
        case .timelineEvent: return "timeline_event"
        case .promotionCandidate: return "promotion_candidate"
        case .unknown(let value): return value
        }
    }

    var displayName: String {
        switch self {
        case .recentContext: return "Recent"
        case .recentSummary: return "Summary"
        case .brainPage: return "Brain Page"
        case .brainChunk: return "Brain"
        case .graph: return "Graph"
        case .timelineEvent: return "Timeline"
        case .promotionCandidate: return "Promotion"
        case .unknown(let value): return value.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }

    init(rawValue: String) {
        switch rawValue {
        case "recent_context": self = .recentContext
        case "recent_summary": self = .recentSummary
        case "brain_page": self = .brainPage
        case "brain_chunk": self = .brainChunk
        case "graph": self = .graph
        case "timeline_event": self = .timelineEvent
        case "promotion_candidate": self = .promotionCandidate
        default: self = .unknown(rawValue)
        }
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        self.init(rawValue: try container.decode(String.self))
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

enum RecentContextSource: Codable, Hashable, Sendable {
    case screen
    case app
    case browser
    case repo
    case file
    case terminal
    case system
    case unknown(String)

    var rawValue: String {
        switch self {
        case .screen: return "screen"
        case .app: return "app"
        case .browser: return "browser"
        case .repo: return "repo"
        case .file: return "file"
        case .terminal: return "terminal"
        case .system: return "system"
        case .unknown(let value): return value
        }
    }

    var displayName: String {
        rawValue.replacingOccurrences(of: "_", with: " ").capitalized
    }

    init(rawValue: String) {
        switch rawValue {
        case "screen": self = .screen
        case "app": self = .app
        case "browser": self = .browser
        case "repo": self = .repo
        case "file": self = .file
        case "terminal": self = .terminal
        case "system": self = .system
        default: self = .unknown(rawValue)
        }
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        self.init(rawValue: try container.decode(String.self))
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

enum RelationType: String, Codable, CaseIterable, Sendable {
    case worksOn = "works_on"
    case uses
    case visited
    case opened
    case edited
    case mentionedIn = "mentioned_in"
    case discussedWith = "discussed_with"
    case decidedIn = "decided_in"
    case evidenceFor = "evidence_for"
    case relatedTo = "related_to"
    case dependsOn = "depends_on"
    case blocks
    case belongsTo = "belongs_to"
    case partOf = "part_of"
    case authored
    case created
    case prefers
}

enum EntityType: String, Codable, CaseIterable, Sendable {
    case user
    case person
    case company
    case project
    case app
    case website
    case repo
    case file
    case workflow
    case concept
    case decision
    case task
    case meeting
    case document
    case preference
}

struct Memory: Codable, Identifiable, Sendable {
    let id: String
    let userID: String
    let agentID: String?
    let source: RecentContextSource
    let content: String
    let contentHash: String
    let occurredAt: Date
    let createdAt: Date
    let ttlSeconds: Int?
    let importance: Double?
    let metadata: Metadata

    var displayContext: String {
        metadata.context.isEmpty ? source.displayName : metadata.context
    }

    enum CodingKeys: String, CodingKey {
        case id
        case userID = "user_id"
        case agentID = "agent_id"
        case source
        case content
        case contentHash = "content_hash"
        case occurredAt = "occurred_at"
        case createdAt = "created_at"
        case ttlSeconds = "ttl_seconds"
        case importance
        case metadata
    }

    init(
        id: String,
        content: String,
        userID: String,
        metadata: Metadata,
        createdAt: Date,
        agentID: String? = nil,
        source: RecentContextSource = .screen,
        contentHash: String = "",
        occurredAt: Date? = nil,
        ttlSeconds: Int? = nil,
        importance: Double? = nil
    ) {
        self.id = id
        self.userID = userID
        self.agentID = agentID
        self.source = source
        self.content = content
        self.contentHash = contentHash
        self.occurredAt = occurredAt ?? createdAt
        self.createdAt = createdAt
        self.ttlSeconds = ttlSeconds
        self.importance = importance
        self.metadata = metadata
    }
}

struct Metadata: Codable, Sendable {
    let timestamp: String
    let context: String
    let activities: [String]
    let keyElements: [String]
    let userIntent: String
    let displayNum: Int
    let browser: String?
    let url: String?
    let captureReason: String?
    let visibleTextHash: String?
    let readableTextHash: String?
    let textCaptureMode: String?
    let pageTextSummary: String?
    let browserSourceQuality: String?
    let browserCaptureID: String?
    let browserSchemaVersion: Int?
    let appName: String?
    let windowTitle: String?
    let bundleIdentifier: String?
    let repoPath: String?
    let filePath: String?

    enum CodingKeys: String, CodingKey {
        case timestamp
        case context
        case activities
        case keyElements = "key_elements"
        case userIntent = "user_intent"
        case displayNum = "display_num"
        case browser
        case url
        case captureReason = "capture_reason"
        case visibleTextHash = "visible_text_hash"
        case readableTextHash = "readable_text_hash"
        case textCaptureMode = "text_capture_mode"
        case pageTextSummary = "page_text_summary"
        case browserSourceQuality = "browser_source_quality"
        case browserCaptureID = "browser_capture_id"
        case browserSchemaVersion = "browser_schema_version"
        case appName = "app_name"
        case windowTitle = "window_title"
        case bundleIdentifier = "bundle_identifier"
        case repoPath = "repo_path"
        case filePath = "file_path"
    }

    init(
        timestamp: String,
        context: String,
        activities: [String],
        keyElements: [String],
        userIntent: String,
        displayNum: Int,
        browser: String?,
        url: String?,
        captureReason: String?,
        visibleTextHash: String? = nil,
        readableTextHash: String? = nil,
        textCaptureMode: String? = nil,
        pageTextSummary: String? = nil,
        browserSourceQuality: String? = nil,
        browserCaptureID: String? = nil,
        browserSchemaVersion: Int? = nil,
        appName: String? = nil,
        windowTitle: String? = nil,
        bundleIdentifier: String? = nil,
        repoPath: String? = nil,
        filePath: String? = nil
    ) {
        self.timestamp = timestamp
        self.context = context
        self.activities = activities
        self.keyElements = keyElements
        self.userIntent = userIntent
        self.displayNum = displayNum
        self.browser = browser
        self.url = url
        self.captureReason = captureReason
        self.visibleTextHash = visibleTextHash
        self.readableTextHash = readableTextHash
        self.textCaptureMode = textCaptureMode
        self.pageTextSummary = pageTextSummary
        self.browserSourceQuality = browserSourceQuality
        self.browserCaptureID = browserCaptureID
        self.browserSchemaVersion = browserSchemaVersion
        self.appName = appName
        self.windowTitle = windowTitle
        self.bundleIdentifier = bundleIdentifier
        self.repoPath = repoPath
        self.filePath = filePath
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        timestamp = try container.decodeIfPresent(String.self, forKey: .timestamp) ?? ""
        context = try container.decodeIfPresent(String.self, forKey: .context) ?? ""
        activities = try container.decodeIfPresent([String].self, forKey: .activities) ?? []
        keyElements = try container.decodeIfPresent([String].self, forKey: .keyElements) ?? []
        userIntent = try container.decodeIfPresent(String.self, forKey: .userIntent) ?? ""
        displayNum = try container.decodeIfPresent(Int.self, forKey: .displayNum) ?? 0
        browser = try container.decodeIfPresent(String.self, forKey: .browser)
        url = try container.decodeIfPresent(String.self, forKey: .url)
        captureReason = try container.decodeIfPresent(String.self, forKey: .captureReason)
        visibleTextHash = try container.decodeIfPresent(String.self, forKey: .visibleTextHash)
        readableTextHash = try container.decodeIfPresent(String.self, forKey: .readableTextHash)
        textCaptureMode = try container.decodeIfPresent(String.self, forKey: .textCaptureMode)
        pageTextSummary = try container.decodeIfPresent(String.self, forKey: .pageTextSummary)
        browserSourceQuality = try container.decodeIfPresent(String.self, forKey: .browserSourceQuality)
        browserCaptureID = try container.decodeIfPresent(String.self, forKey: .browserCaptureID)
        browserSchemaVersion = try container.decodeIfPresent(Int.self, forKey: .browserSchemaVersion)
        appName = try container.decodeIfPresent(String.self, forKey: .appName)
        windowTitle = try container.decodeIfPresent(String.self, forKey: .windowTitle)
        bundleIdentifier = try container.decodeIfPresent(String.self, forKey: .bundleIdentifier)
        repoPath = try container.decodeIfPresent(String.self, forKey: .repoPath)
        filePath = try container.decodeIfPresent(String.self, forKey: .filePath)
    }
}

struct Evidence: Codable, Hashable, Sendable {
    let source: MemorySource
    let sourceID: String
    let excerpt: String?
    let contentHash: String?
    let createdAt: Date?
    let metadata: [String: JSONValue]?

    enum CodingKeys: String, CodingKey {
        case source
        case sourceID = "source_id"
        case excerpt
        case contentHash = "content_hash"
        case createdAt = "created_at"
        case metadata
    }
}

struct MemoryRelation: Codable, Identifiable, Hashable, Sendable {
    let id: String
    let relationType: RelationType
    let sourceEntityID: String
    let targetEntityID: String
    let confidence: Double
    let evidence: [Evidence]

    enum CodingKeys: String, CodingKey {
        case id
        case relationType = "relation_type"
        case sourceEntityID = "source_entity_id"
        case targetEntityID = "target_entity_id"
        case confidence
        case evidence
    }
}

struct MemoryScoreBreakdown: Codable, Hashable, Sendable {
    let vector: Double
    let keyword: Double
    let graph: Double
    let recency: Double
}

struct SearchResult: Codable, Identifiable, Hashable, Sendable {
    let id: String
    let source: MemorySource
    let content: String
    let userID: String
    let entityIDs: [String]
    let relations: [MemoryRelation]
    let evidence: [Evidence]
    let score: Double
    let scores: MemoryScoreBreakdown
    let createdAt: Date
    let metadata: [String: JSONValue]

    var displayContext: String {
        if let slug = metadata["slug"]?.stringValue {
            return "\(source.displayName): \(slug)"
        }
        if let chunkType = metadata["chunk_type"]?.stringValue {
            return "\(source.displayName): \(chunkType)"
        }
        return source.displayName
    }

    enum CodingKeys: String, CodingKey {
        case id
        case source
        case content
        case userID = "user_id"
        case entityIDs = "entity_ids"
        case relations
        case evidence
        case score
        case scores
        case createdAt = "created_at"
        case metadata
    }
}

struct EnhancementResult: Sendable {
    let originalPrompt: String
    let enhancedPrompt: String
    let memoriesUsed: [String]
    let memoryCount: Int
    let enhancementType: String
}

struct MemoryInfo: Codable, Sendable {
    let id: String
    let content: String
    let context: String
    let score: Double
    let date: Date
}

struct RecentContextEventInput: Encodable, Sendable {
    let userID: String
    let agentID: String?
    let idempotencyKey: String
    let source: RecentContextSource
    let content: String
    let occurredAt: String
    let ttlSeconds: Int
    let importance: Double
    let metadata: Metadata

    enum CodingKeys: String, CodingKey {
        case userID = "user_id"
        case agentID = "agent_id"
        case idempotencyKey = "idempotency_key"
        case source
        case content
        case occurredAt = "occurred_at"
        case ttlSeconds = "ttl_seconds"
        case importance
        case metadata
    }
}

struct RecentContextEventResponse: Decodable, Sendable {
    let schemaVersion: String
    let event: Memory

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case event
    }
}

struct RecentContextListResponse: Decodable, Sendable {
    let schemaVersion: String
    let items: [Memory]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case items
    }
}

struct SearchMemoryRequest: Encodable, Sendable {
    let query: String
    let userID: String
    let agentID: String?
    let scopes: [MemorySource]?
    let limit: Int
    let debug: Bool

    enum CodingKeys: String, CodingKey {
        case query
        case userID = "user_id"
        case agentID = "agent_id"
        case scopes
        case limit
        case debug
    }
}

struct SearchMemoryResponse: Decodable, Sendable {
    let schemaVersion: String
    let query: String
    let items: [SearchResult]
    let debug: SearchDebug

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case query
        case items
        case debug
    }
}

struct SearchDebug: Codable, Hashable, Sendable {
    let matchedEntities: [String]
    let ranking: [String: JSONValue]

    enum CodingKeys: String, CodingKey {
        case matchedEntities = "matched_entities"
        case ranking
    }
}

struct HealthResponse: Decodable, Sendable {
    let schemaVersion: String
    let status: String

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case status
    }
}

struct TimeWindow: Codable, Hashable, Sendable {
    let startedAt: Date
    let endedAt: Date

    enum CodingKeys: String, CodingKey {
        case startedAt = "started_at"
        case endedAt = "ended_at"
    }
}

struct CurrentContextPacket: Decodable, Sendable {
    let schemaVersion: String
    let userID: String
    let agentID: String?
    let generatedAt: Date
    let window: TimeWindow
    let summary: String
    let recentEvents: [Memory]
    let activeEntities: [String]
    let metadata: [String: JSONValue]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case userID = "user_id"
        case agentID = "agent_id"
        case generatedAt = "generated_at"
        case window
        case summary
        case recentEvents = "recent_events"
        case activeEntities = "active_entities"
        case metadata
    }
}

struct GraphNode: Codable, Identifiable, Hashable, Sendable {
    let id: String
    let type: EntityType
    let name: String
    let aliases: [String]
    let metadata: [String: JSONValue]
}

struct GraphQueryResponse: Decodable, Sendable {
    let schemaVersion: String
    let start: String
    let nodes: [GraphNode]
    let relations: [MemoryRelation]
    let depth: Int
    let direction: String
    let generatedAt: Date

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case start
        case nodes
        case relations
        case depth
        case direction
        case generatedAt = "generated_at"
    }
}

struct MemoryJobRef: Codable, Hashable, Sendable {
    let id: String
    let status: String
    let idempotencyKey: String

    enum CodingKeys: String, CodingKey {
        case id
        case status
        case idempotencyKey = "idempotency_key"
    }
}

struct BrainSyncResponse: Decodable, Sendable {
    let schemaVersion: String
    let job: MemoryJobRef
    let syncedPages: [BrainPageRef]
    let errors: [BrainSyncError]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case job
        case syncedPages = "synced_pages"
        case errors
    }
}

struct BrainPageRef: Codable, Identifiable, Hashable, Sendable {
    let id: String
    let slug: String
    let path: String
    let type: EntityType
    let title: String
    let sourceHash: String
    let updatedAt: Date

    enum CodingKeys: String, CodingKey {
        case id
        case slug
        case path
        case type
        case title
        case sourceHash = "source_hash"
        case updatedAt = "updated_at"
    }
}

struct BrainSyncError: Codable, Hashable, Sendable {
    let path: String
    let code: String
    let message: String
}

struct PromotionResponse: Decodable, Sendable {
    let schemaVersion: String
    let candidateID: String
    let mode: String
    let status: String
    let targetSlug: String
    let suggestedEdit: String
    let evidence: [Evidence]
    let metadata: [String: JSONValue]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case candidateID = "candidate_id"
        case mode
        case status
        case targetSlug = "target_slug"
        case suggestedEdit = "suggested_edit"
        case evidence
        case metadata
    }
}

struct DeleteResponse: Decodable, Sendable {
    let schemaVersion: String
    let deleted: Bool
    let source: MemorySource
    let id: String
    let generatedAt: Date

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case deleted
        case source
        case id
        case generatedAt = "generated_at"
    }
}

enum JSONValue: Codable, Hashable, Sendable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case object([String: JSONValue])
    case array([JSONValue])
    case null

    var stringValue: String? {
        if case .string(let value) = self {
            return value
        }
        return nil
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()

        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()

        switch self {
        case .string(let value):
            try container.encode(value)
        case .number(let value):
            try container.encode(value)
        case .bool(let value):
            try container.encode(value)
        case .object(let value):
            try container.encode(value)
        case .array(let value):
            try container.encode(value)
        case .null:
            try container.encodeNil()
        }
    }
}
