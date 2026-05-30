import CryptoKit
import Foundation

actor MemoryService {
    private let config: MemoryConfig
    private let session: URLSession
    private let decoder: JSONDecoder
    private let encoder: JSONEncoder

    init(config: MemoryConfig) {
        self.config = config
        self.session = URLSession.shared
        self.decoder = MemoryV2JSON.makeDecoder()
        self.encoder = MemoryV2JSON.makeEncoder()
    }

    func add(content: String, metadata: Metadata) async throws -> Memory {
        let payload = RecentContextEventInput(
            userID: config.userID,
            agentID: config.collectionName,
            idempotencyKey: idempotencyKey(for: content, metadata: metadata),
            source: inferSource(from: metadata),
            content: content,
            occurredAt: metadata.timestamp.isEmpty ? ISO8601DateFormatter().string(from: Date()) : metadata.timestamp,
            ttlSeconds: 6 * 60 * 60,
            importance: 0.5,
            metadata: metadata
        )

        var request = try makeRequest(path: "/v2/recent-context", method: "POST")
        request.httpBody = try encoder.encode(payload)

        let data = try await performRequest(request)
        let response = try decoder.decode(RecentContextEventResponse.self, from: data)
        try validateSchemaVersion(response.schemaVersion)
        return response.event
    }

    func search(query: String, limit: Int = 10) async throws -> [SearchResult] {
        let payload = SearchMemoryRequest(
            query: query,
            userID: config.userID,
            agentID: config.collectionName,
            scopes: nil,
            limit: limit,
            debug: false
        )

        var request = try makeRequest(path: "/v2/search", method: "POST")
        request.httpBody = try encoder.encode(payload)

        let data = try await performRequest(request)
        let response = try decoder.decode(SearchMemoryResponse.self, from: data)
        try validateSchemaVersion(response.schemaVersion)
        return response.items
    }

    func getRecent(limit: Int = 10) async throws -> [Memory] {
        var queryItems = [
            URLQueryItem(name: "user_id", value: config.userID),
            URLQueryItem(name: "limit", value: String(limit))
        ]

        if !config.collectionName.isEmpty {
            queryItems.append(URLQueryItem(name: "agent_id", value: config.collectionName))
        }

        let request = try makeRequest(path: "/v2/recent-context", queryItems: queryItems)
        let data = try await performRequest(request)
        let response = try decoder.decode(RecentContextListResponse.self, from: data)
        try validateSchemaVersion(response.schemaVersion)
        return response.items
    }

    func getCurrentContext() async throws -> CurrentContextPacket {
        var queryItems = [
            URLQueryItem(name: "user_id", value: config.userID)
        ]

        if !config.collectionName.isEmpty {
            queryItems.append(URLQueryItem(name: "agent_id", value: config.collectionName))
        }

        let request = try makeRequest(path: "/v2/current-context", queryItems: queryItems)
        let data = try await performRequest(request)
        let response = try decoder.decode(CurrentContextPacket.self, from: data)
        try validateSchemaVersion(response.schemaVersion)
        return response
    }

    func delete(memoryID: String, source: MemorySource = .recentContext) async throws {
        let encodedSource = source.rawValue.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? source.rawValue
        let encodedID = memoryID.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? memoryID
        let queryItems = [
            URLQueryItem(name: "user_id", value: config.userID)
        ]

        let request = try makeRequest(
            path: "/v2/memories/\(encodedSource)/\(encodedID)",
            queryItems: queryItems,
            method: "DELETE"
        )

        _ = try await performRequest(request, allowsEmptyBody: true)
    }

    func checkHealth() async -> Bool {
        do {
            let request = try makeRequest(path: "/v2/health")
            let data = try await performRequest(request)
            let response = try decoder.decode(HealthResponse.self, from: data)
            return response.schemaVersion == MemoryV2JSON.schemaVersion && response.status == "ok"
        } catch {
            return false
        }
    }

    private func makeRequest(
        path: String,
        queryItems: [URLQueryItem] = [],
        method: String = "GET"
    ) throws -> URLRequest {
        guard var components = URLComponents(string: config.baseURL) else {
            throw URLError(.badURL)
        }

        let normalizedBasePath = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let normalizedPath = path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        components.path = "/" + [normalizedBasePath, normalizedPath]
            .filter { !$0.isEmpty }
            .joined(separator: "/")
        components.queryItems = queryItems.isEmpty ? nil : queryItems

        guard let url = components.url else {
            throw URLError(.badURL)
        }

        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")

        if !config.apiKey.isEmpty {
            request.setValue("Bearer \(config.apiKey)", forHTTPHeaderField: "Authorization")
        }

        return request
    }

    private func performRequest(_ request: URLRequest, allowsEmptyBody: Bool = false) async throws -> Data {
        let (data, response) = try await session.data(for: request)

        guard let httpResponse = response as? HTTPURLResponse else {
            throw URLError(.badServerResponse)
        }

        guard (200..<300).contains(httpResponse.statusCode) else {
            if let apiError = try? decoder.decode(MemoryV2ErrorResponse.self, from: data) {
                throw MemoryServiceError.apiError(apiError.error.message)
            }
            if let legacyError = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               let message = legacyError["error"] as? String {
                throw MemoryServiceError.apiError(message)
            }
            throw URLError(.badServerResponse)
        }

        if data.isEmpty && !allowsEmptyBody {
            throw URLError(.zeroByteResource)
        }

        return data
    }

    private func validateSchemaVersion(_ schemaVersion: String) throws {
        guard schemaVersion == MemoryV2JSON.schemaVersion else {
            throw MemoryServiceError.schemaMismatch(schemaVersion)
        }
    }

    private func inferSource(from metadata: Metadata) -> RecentContextSource {
        if metadata.url != nil || metadata.browser != nil {
            return .browser
        }

        let reason = metadata.captureReason?.lowercased() ?? ""
        let context = metadata.context.lowercased()

        if reason.contains("terminal") || context.contains("terminal") {
            return .terminal
        }
        if reason.contains("file") || context.contains("file") || context.contains("document") {
            return .file
        }
        if reason.contains("repo") || context.contains("code") {
            return .repo
        }
        if reason.contains("app") || context.contains("meeting") || metadata.appName != nil {
            return .app
        }

        return .screen
    }

    private func idempotencyKey(for content: String, metadata: Metadata) -> String {
        let fingerprint = stableFingerprint(
            [
                config.userID,
                config.collectionName,
                metadata.timestamp,
                metadata.context,
                content
            ].joined(separator: "|")
        )

        return "recent_context_\(config.userID)_\(fingerprint)"
    }

    private func stableFingerprint(_ value: String) -> String {
        let digest = SHA256.hash(data: Data(value.utf8))
        return digest.prefix(12).map { String(format: "%02x", $0) }.joined()
    }
}

private struct MemoryV2ErrorResponse: Decodable {
    let schemaVersion: String
    let error: MemoryV2Error

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case error
    }
}

private struct MemoryV2Error: Decodable {
    let code: String
    let message: String
}

enum MemoryServiceError: LocalizedError {
    case apiError(String)
    case schemaMismatch(String)

    var errorDescription: String? {
        switch self {
        case .apiError(let message):
            return message
        case .schemaMismatch(let version):
            return "Expected Memory v2 schema version, got \(version)"
        }
    }
}
