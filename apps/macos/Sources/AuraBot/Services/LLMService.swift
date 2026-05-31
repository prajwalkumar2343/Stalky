import Foundation

actor LLMService {
    private let config: LLMConfig
    private let session: URLSession
    
    init(config: LLMConfig) {
        self.config = config
        let sessionConfiguration = URLSessionConfiguration.default
        sessionConfiguration.timeoutIntervalForRequest = TimeInterval(config.timeoutSeconds)
        self.session = URLSession(configuration: sessionConfiguration)
    }
    
    func analyzeScreen(imageData: Data, context: String) async throws -> AnalysisResult {
        try LLMRequestValidator.validateImageData(imageData)
        let base64Image = imageData.base64EncodedString()
        
        let messages: [[String: Any]] = [
            [
                "role": "system",
                "content": "You are a screen analysis assistant. Analyze the screenshot and describe what the user is doing."
            ],
            [
                "role": "user",
                "content": [
                    [
                        "type": "text",
                        "text": "Analyze this screenshot. Context from previous memories: \(context)"
                    ],
                    [
                        "type": "image_url",
                        "image_url": [
                            "url": "data:image/jpeg;base64,\(base64Image)"
                        ]
                    ]
                ]
            ]
        ]
        
        let requestBody: [String: Any] = [
            "model": config.model,
            "messages": messages,
            "max_tokens": config.maxTokens,
            "temperature": config.temperature
        ]
        
        let result = try await makeRequest(body: requestBody)
        return parseAnalysisResult(result)
    }
    
    func generateResponse(message: String, memories: [String]) async throws -> String {
        let normalizedMessage = try LLMRequestValidator.normalizedRequiredText(message, fieldName: "Message")
        let context = memories
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .joined(separator: "\n")
        
        let messages: [[String: Any]] = [
            [
                "role": "system",
                "content": "You are a helpful assistant with access to the user's memory. Use the provided context to answer questions."
            ],
            [
                "role": "user",
                "content": "Context from user's memories:\n\(context)\n\nUser question: \(normalizedMessage)"
            ]
        ]
        
        let requestBody: [String: Any] = [
            "model": config.openRouterChatModel,
            "messages": messages,
            "max_tokens": config.maxTokens,
            "temperature": config.temperature
        ]
        
        let result = try await makeRequest(body: requestBody)
        return result
    }
    
    func enhancePrompt(prompt: String, memories: [MemoryInfo]) async throws -> EnhancementResult {
        let normalizedPrompt = try LLMRequestValidator.normalizedRequiredText(prompt, fieldName: "Prompt")
        let memoriesText = memories.map { "- [\($0.context)] \($0.content)" }.joined(separator: "\n")
        
        let messages: [[String: Any]] = [
            [
                "role": "system",
                "content": "Enhance the user's prompt with relevant context from their memory. Keep the original intent but add helpful context."
            ],
            [
                "role": "user",
                "content": """
                Original prompt: \(normalizedPrompt)
                
                Relevant memories:
                \(memoriesText)
                
                Enhance the prompt with relevant context. Return ONLY the enhanced prompt.
                """
            ]
        ]
        
        let requestBody: [String: Any] = [
            "model": config.openRouterChatModel,
            "messages": messages,
            "max_tokens": config.maxTokens,
            "temperature": 0.5
        ]
        
        let enhanced = try await makeRequest(body: requestBody)
        
        return EnhancementResult(
            originalPrompt: prompt,
            enhancedPrompt: enhanced,
            memoriesUsed: memories.map { $0.content },
            memoryCount: memories.count,
            enhancementType: memories.count > 2 ? "contextual" : memories.count > 0 ? "detailed" : "minimal"
        )
    }
    
    func checkHealth() async -> Bool {
        guard let url = LLMRequestValidator.endpoint(baseURL: config.baseURL, path: "models") else { return false }
        
        do {
            let (_, response) = try await session.data(from: url)
            return (response as? HTTPURLResponse)?.statusCode == 200
        } catch {
            return false
        }
    }
    
    private func makeRequest(body: [String: Any]) async throws -> String {
        guard let url = LLMRequestValidator.endpoint(baseURL: config.baseURL, path: "chat/completions") else {
            throw LLMServiceError.invalidBaseURL(config.baseURL)
        }
        
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if !config.openRouterAPIKey.isEmpty {
            request.setValue("Bearer \(config.openRouterAPIKey)", forHTTPHeaderField: "Authorization")
        }
        
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        
        let (data, response) = try await session.data(for: request)
        
        guard let httpResponse = response as? HTTPURLResponse else {
            throw LLMServiceError.invalidResponse
        }

        guard httpResponse.statusCode == 200 else {
            throw LLMServiceError.httpError(
                statusCode: httpResponse.statusCode,
                message: Self.apiErrorMessage(from: data)
            )
        }
        
        guard let json = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let choices = json["choices"] as? [[String: Any]],
              let first = choices.first,
              let message = first["message"] as? [String: Any],
              let content = message["content"] as? String else {
            throw LLMServiceError.invalidResponse
        }
        
        return content
    }
    
    private func parseAnalysisResult(_ text: String) -> AnalysisResult {
        return AnalysisResult(
            summary: text,
            context: "Screen capture",
            activities: ["Working"],
            keyElements: [],
            userIntent: "Productivity"
        )
    }

    private static func apiErrorMessage(from data: Data) -> String? {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return nil
        }

        if let error = json["error"] as? [String: Any] {
            return error["message"] as? String ?? error["code"] as? String
        }

        return json["message"] as? String
    }
}

enum LLMServiceError: LocalizedError, Equatable {
    case emptyInput(String)
    case emptyImage
    case invalidBaseURL(String)
    case httpError(statusCode: Int, message: String?)
    case invalidResponse

    var errorDescription: String? {
        switch self {
        case .emptyInput(let fieldName):
            return "\(fieldName) cannot be empty."
        case .emptyImage:
            return "Screen analysis needs a non-empty screenshot."
        case .invalidBaseURL:
            return "LLM Base URL is invalid. Check Settings and use an http or https URL."
        case .httpError(let statusCode, let message):
            if let message, !message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                return "LLM request failed (\(statusCode)): \(message)"
            }
            return "LLM request failed with HTTP \(statusCode)."
        case .invalidResponse:
            return "LLM returned an unexpected response."
        }
    }
}

enum LLMRequestValidator {
    static func normalizedRequiredText(_ value: String, fieldName: String) throws -> String {
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalized.isEmpty else {
            throw LLMServiceError.emptyInput(fieldName)
        }
        return normalized
    }

    static func validateImageData(_ data: Data) throws {
        guard !data.isEmpty else {
            throw LLMServiceError.emptyImage
        }
    }

    static func endpoint(baseURL: String, path: String) -> URL? {
        guard var components = URLComponents(string: baseURL.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = components.scheme?.lowercased(),
              ["http", "https"].contains(scheme),
              components.host?.isEmpty == false else {
            return nil
        }

        let basePath = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        let endpointPath = path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        components.path = "/" + [basePath, endpointPath].filter { !$0.isEmpty }.joined(separator: "/")
        components.query = nil
        components.fragment = nil
        return components.url
    }
}

enum LLMFailureMessage {
    static func describe(_ error: Error) -> String {
        if let llmError = error as? LLMServiceError {
            return llmError.errorDescription ?? "LLM request failed."
        }

        if let urlError = error as? URLError {
            switch urlError.code {
            case .badURL, .unsupportedURL:
                return "LLM Base URL is invalid. Check Settings and use an http or https URL."
            case .cannotFindHost, .cannotConnectToHost, .networkConnectionLost, .notConnectedToInternet, .timedOut:
                return "LLM service is unreachable. Check your network, API endpoint, or OpenRouter status."
            case .cannotParseResponse, .badServerResponse:
                return "LLM service returned an unexpected response."
            default:
                break
            }
        }

        let description = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        return description.isEmpty ? "LLM request failed unexpectedly." : "LLM request failed: \(description)"
    }
}
