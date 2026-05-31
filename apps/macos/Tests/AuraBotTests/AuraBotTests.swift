import Foundation
import XCTest
@testable import AuraBot

@available(macOS 14.0, *)
final class AuraBotCoreTests: XCTestCase {
    func testPluginHostActivatesWorkspaceTakeoverPolicies() throws {
        let host = PluginHost()
        let descriptor = makeWorkspacePluginDescriptor()

        try host.activateWorkspace(descriptor)

        XCTAssertEqual(host.activeWorkspacePlugin?.pluginID, "com.aurabot.ai-tutor")
        XCTAssertEqual(
            host.activeAppPresentation,
            AppPresentationPolicy(mode: .pluginWorkspace(pluginID: "com.aurabot.ai-tutor", name: "AI Tutor"))
        )
        XCTAssertEqual(host.activeCapturePolicy.priority, [.browserDOM, .browserTranscript, .appMetadata])
        XCTAssertEqual(host.activeWindowPolicy.presentation, .floatingOverlay)
    }

    func testPluginHostReturnsHostDefaultsAfterWorkspaceDeactivation() throws {
        let host = PluginHost()

        try host.activateWorkspace(makeWorkspacePluginDescriptor())
        host.deactivateWorkspace(pluginID: "com.aurabot.ai-tutor")

        XCTAssertNil(host.activeWorkspacePlugin)
        XCTAssertEqual(host.activeAppPresentation, .hostDefault)
        XCTAssertEqual(host.activeCapturePolicy, .hostDefault)
        XCTAssertEqual(host.activeWindowPolicy, .hostDefault)
    }

    func testPluginHostIgnoresDeactivateForDifferentPlugin() throws {
        let host = PluginHost()

        try host.activateWorkspace(makeWorkspacePluginDescriptor())
        host.deactivateWorkspace(pluginID: "com.example.other")

        XCTAssertEqual(host.activeWorkspacePlugin?.pluginID, "com.aurabot.ai-tutor")
    }

    @MainActor
    func testOnboardingGateUsesPersistedCompletionOnly() {
        var incompleteConfig = AppConfig.default
        incompleteConfig.app.onboardingCompleted = false
        XCTAssertTrue(AppService(config: incompleteConfig).needsOnboarding)

        var completedConfig = AppConfig.default
        completedConfig.app.onboardingCompleted = true
        XCTAssertFalse(AppService(config: completedConfig).needsOnboarding)
    }

    func testCapturePolicyDisablesVisualFallbackWhenPluginRequestsStructuredOnlyCapture() {
        let policy = CapturePolicy(
            priority: [.browserDOM, .browserTranscript, .appMetadata],
            fallback: .none,
            redaction: .strict
        )

        XCTAssertTrue(policy.allowsBrowserContext)
        XCTAssertTrue(policy.allowsAppMetadata)
        XCTAssertFalse(policy.allowsVisualFallback)
    }

    @MainActor
    func testAppServiceAppliesWorkspacePluginPresentationAndRollback() async throws {
        let service = AppService()

        try await service.activateWorkspacePlugin(makeWorkspacePluginDescriptor())

        XCTAssertEqual(
            service.appPresentation,
            AppPresentationPolicy(mode: .pluginWorkspace(pluginID: "com.aurabot.ai-tutor", name: "AI Tutor"))
        )
        XCTAssertEqual(service.windowPolicy.presentation, .floatingOverlay)

        await service.deactivateWorkspacePlugin(pluginID: "com.aurabot.ai-tutor")

        XCTAssertEqual(service.appPresentation, .hostDefault)
        XCTAssertEqual(service.windowPolicy, .hostDefault)
    }

    func testMemoryV2SearchFixtureDecodes() throws {
        let response = try decodeMemoryFixture("search-response", as: SearchMemoryResponse.self)

        XCTAssertEqual(response.schemaVersion, MemoryV2JSON.schemaVersion)
        XCTAssertEqual(response.items.first?.source, .brainChunk)
        XCTAssertEqual(response.items.first?.relations.first?.relationType, .decidedIn)
        XCTAssertEqual(response.debug.matchedEntities.first, "ent_project_aurabot")
    }

    func testMemoryV2RecentContextFixturesDecode() throws {
        let eventResponse = try decodeMemoryFixture(
            "recent-context-event-response",
            as: RecentContextEventResponse.self
        )
        let listResponse = try decodeMemoryFixture(
            "recent-context-list-response",
            as: RecentContextListResponse.self
        )

        XCTAssertEqual(eventResponse.schemaVersion, MemoryV2JSON.schemaVersion)
        XCTAssertEqual(eventResponse.event.source, .browser)
        XCTAssertEqual(eventResponse.event.displayContext, "Browse")
        XCTAssertEqual(listResponse.items.count, 2)
        XCTAssertEqual(listResponse.items.last?.source, .repo)
    }

    func testMemoryV2OperationalFixturesDecode() throws {
        let currentContext = try decodeMemoryFixture(
            "current-context-response",
            as: CurrentContextPacket.self
        )
        let graph = try decodeMemoryFixture("graph-query-response", as: GraphQueryResponse.self)
        let brainSync = try decodeMemoryFixture("brain-sync-response", as: BrainSyncResponse.self)
        let promotion = try decodeMemoryFixture("promotion-response", as: PromotionResponse.self)
        let delete = try decodeMemoryFixture("delete-response", as: DeleteResponse.self)
        let health = try decodeMemoryFixture("health-response", as: HealthResponse.self)

        XCTAssertEqual(currentContext.activeEntities, ["ent_project_aurabot", "ent_concept_memory_v2"])
        XCTAssertEqual(graph.relations.first?.evidence.first?.source, .brainPage)
        XCTAssertEqual(brainSync.syncedPages.first?.slug, "projects/aurabot")
        XCTAssertEqual(promotion.mode, "draft")
        XCTAssertEqual(delete.source, .recentContext)
        XCTAssertEqual(health.status, "ok")
    }

    func testMemoryFailureMessagesAreActionable() {
        XCTAssertEqual(
            MemoryFailureMessage.describe(MemoryServiceError.apiError("invalid payload")),
            "Memory service rejected the request: invalid payload"
        )
        XCTAssertTrue(
            MemoryFailureMessage.describe(MemoryServiceError.schemaMismatch("memory-v1"))
                .contains("schema mismatch")
        )
        XCTAssertTrue(
            MemoryFailureMessage.describe(URLError(.badURL))
                .contains("Memory service URL is invalid")
        )
        XCTAssertTrue(
            MemoryFailureMessage.describe(URLError(.cannotConnectToHost))
                .contains("unreachable")
        )
        XCTAssertTrue(
            MemoryFailureMessage.describe(URLError(.zeroByteResource))
                .contains("empty response")
        )
    }

    func testLLMRequestValidatorNormalizesInputsAndBuildsSafeEndpoints() throws {
        XCTAssertEqual(
            try LLMRequestValidator.normalizedRequiredText("  hello  ", fieldName: "Message"),
            "hello"
        )
        XCTAssertThrowsError(
            try LLMRequestValidator.normalizedRequiredText("   ", fieldName: "Message")
        )
        XCTAssertNoThrow(try LLMRequestValidator.validateImageData(Data([1, 2, 3])))
        XCTAssertThrowsError(try LLMRequestValidator.validateImageData(Data()))

        XCTAssertEqual(
            LLMRequestValidator.endpoint(
                baseURL: " https://openrouter.ai/api/v1/ ",
                path: "/chat/completions"
            )?.absoluteString,
            "https://openrouter.ai/api/v1/chat/completions"
        )
        XCTAssertNil(LLMRequestValidator.endpoint(baseURL: "not a url", path: "models"))
        XCTAssertNil(LLMRequestValidator.endpoint(baseURL: "ftp://example.com", path: "models"))
    }

    func testLLMFailureMessagesAreActionable() {
        XCTAssertEqual(
            LLMFailureMessage.describe(LLMServiceError.emptyInput("Prompt")),
            "Prompt cannot be empty."
        )
        XCTAssertEqual(
            LLMFailureMessage.describe(LLMServiceError.emptyImage),
            "Screen analysis needs a non-empty screenshot."
        )
        XCTAssertTrue(
            LLMFailureMessage.describe(
                LLMServiceError.httpError(statusCode: 401, message: "invalid api key")
            ).contains("invalid api key")
        )
        XCTAssertTrue(
            LLMFailureMessage.describe(URLError(.cannotConnectToHost))
                .contains("unreachable")
        )
        XCTAssertTrue(
            LLMFailureMessage.describe(URLError(.cannotParseResponse))
                .contains("unexpected response")
        )
    }

    func testYouTubeWatchURLDerivesStableMediaContext() {
        let derived = BrowserContextService.deriveActivity(
            url: "https://www.youtube.com/watch?v=abc123&t=30",
            title: "Demo video"
        )

        XCTAssertEqual(derived.activity, .media)
        XCTAssertEqual(derived.mediaID, "abc123")
        XCTAssertEqual(derived.pageID, "www.youtube.com/watch")
    }

    func testNormalizedPageIDIgnoresQueryAndFragment() throws {
        let components = try XCTUnwrap(
            URLComponents(string: "https://Example.com/docs/page?utm_source=test#intro")
        )

        XCTAssertEqual(
            BrowserContextService.normalizedPageID(for: components),
            "example.com/docs/page"
        )
    }

    func testBrowserActivityDerivationIgnoresWhitespaceOnlyFallbacks() {
        let derived = BrowserContextService.deriveActivity(url: "   ", title: "   ")

        XCTAssertEqual(derived.activity, .browsing)
        XCTAssertNil(derived.pageID)
        XCTAssertNil(derived.mediaID)
    }

    func testBrowserContextFreshnessRejectsFarFutureTimestamps() {
        let now = Date(timeIntervalSince1970: 1_000)

        XCTAssertTrue(
            BrowserContextService.isContextFresh(
                timestamp: now.addingTimeInterval(30),
                now: now,
                freshnessSeconds: 60
            )
        )
        XCTAssertFalse(
            BrowserContextService.isContextFresh(
                timestamp: now.addingTimeInterval(120),
                now: now,
                freshnessSeconds: 60
            )
        )
        XCTAssertFalse(
            BrowserContextService.isContextFresh(
                timestamp: now.addingTimeInterval(-61),
                now: now,
                freshnessSeconds: 60
            )
        )
    }

    func testBrowserContextTimestampClampsImpossibleFuturePayloads() {
        let now = Date(timeIntervalSince1970: 1_000)

        XCTAssertEqual(
            BrowserContextService.normalizedContextTimestamp(
                now.addingTimeInterval(30),
                now: now
            ),
            now.addingTimeInterval(30)
        )
        XCTAssertEqual(
            BrowserContextService.normalizedContextTimestamp(
                now.addingTimeInterval(120),
                now: now
            ),
            now
        )
        XCTAssertEqual(BrowserContextService.normalizedContextTimestamp(nil, now: now), now)
    }

    func testBrowserContextSessionKeyPrefersMediaThenPage() {
        let context = BrowserContext(
            source: .extensionData,
            browser: "Chrome",
            bundleIdentifier: "com.google.Chrome",
            url: "https://example.com/watch",
            title: "Video",
            activity: .media,
            pageID: "example.com/watch",
            mediaID: "video-1",
            mediaIsPlaying: true,
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
            timestamp: Date(timeIntervalSince1970: 0)
        )

        XCTAssertEqual(context.sessionKey, "media:video-1")
        XCTAssertTrue(context.llmSummary.contains("Activity: media playback"))
    }

    func testBrowserContextRouterFingerprintIncludesTextHashesAndCaptureMode() {
        let base = makeBrowserContext(
            source: .extensionData,
            browser: "Google Chrome",
            bundleIdentifier: "com.google.Chrome",
            url: "https://example.com/docs",
            title: "Example Docs",
            visibleTextHash: "visible-v1",
            readableTextHash: "readable-v1",
            textCaptureMode: "full_readable_text"
        )
        let changedVisibleText = makeBrowserContext(
            source: .extensionData,
            browser: "Google Chrome",
            bundleIdentifier: "com.google.Chrome",
            url: "https://example.com/docs",
            title: "Example Docs",
            visibleTextHash: "visible-v2",
            readableTextHash: "readable-v1",
            textCaptureMode: "full_readable_text"
        )
        let metadataOnly = makeBrowserContext(
            source: .extensionData,
            browser: "Google Chrome",
            bundleIdentifier: "com.google.Chrome",
            url: "https://example.com/docs",
            title: "Example Docs",
            visibleTextHash: "visible-v1",
            readableTextHash: "readable-v1",
            textCaptureMode: "sensitive_metadata_only"
        )

        XCTAssertNotEqual(
            ContextRouter.browserEventFingerprint(for: base),
            ContextRouter.browserEventFingerprint(for: changedVisibleText)
        )
        XCTAssertNotEqual(
            ContextRouter.browserEventFingerprint(for: base),
            ContextRouter.browserEventFingerprint(for: metadataOnly)
        )
    }

    func testExtensionConfigDecodesLegacyPayloadWithSecureDefaults() throws {
        let payload = """
        {
          "enabled": true,
          "port": 7345,
          "freshnessSeconds": 15
        }
        """.data(using: .utf8)!

        let config = try JSONDecoder().decode(ExtensionConfig.self, from: payload)

        XCTAssertEqual(config.apiKey, "")
        XCTAssertTrue(config.allowedOrigins.contains("chrome-extension://"))
        XCTAssertTrue(config.allowedOrigins.contains("http://127.0.0.1:"))
    }

    func testContextCollectorRewritePolicyRequiresOptIn() {
        let config = LLMConfig()

        XCTAssertFalse(config.allowsContextCollectorRewrite(for: "google/gemini-3.1-pro"))
    }

    func testContextCollectorRewritePolicyAcceptsConfiguredModelThresholds() {
        var config = LLMConfig()
        config.contextCollectorRewrite.enabled = true

        XCTAssertTrue(config.allowsContextCollectorRewrite(for: "google/gemini-3.1-pro"))
        XCTAssertTrue(config.allowsContextCollectorRewrite(for: "anthropic/claude-opus-4.5"))
        XCTAssertTrue(config.allowsContextCollectorRewrite(for: "openai/gpt-5.3"))
        XCTAssertTrue(config.allowsContextCollectorRewrite(for: "moonshotai/kimi-2.5"))
    }

    func testContextCollectorRewritePolicyRejectsLowerOrWrongTierModels() {
        var config = LLMConfig()
        config.contextCollectorRewrite.enabled = true

        XCTAssertFalse(config.allowsContextCollectorRewrite(for: "google/gemini-2.5-pro"))
        XCTAssertFalse(config.allowsContextCollectorRewrite(for: "anthropic/claude-sonnet-4.5"))
        XCTAssertFalse(config.allowsContextCollectorRewrite(for: "openai/gpt-5.2"))
        XCTAssertFalse(config.allowsContextCollectorRewrite(for: "moonshotai/kimi-2.1"))
    }

    func testBrowserExtensionPayloadCapturesTextHashesWithoutPersistingPrivateWindowText() throws {
        let payload = BrowserExtensionUpdateRequest(
            schemaVersion: 1,
            captureID: "capture-1",
            browser: "Google Chrome",
            bundleIdentifier: "com.google.Chrome",
            url: "https://example.com/article",
            title: "Example Article",
            activity: .browsing,
            pageID: nil,
            mediaID: nil,
            mediaIsPlaying: nil,
            scrollPercent: 42,
            viewportSignature: "viewport-1",
            noveltyScore: 0.6,
            visibleText: "Visible article text",
            selectedText: "Selected sentence",
            readableText: "Full readable page text",
            visibleTextHash: "visible-hash",
            readableTextHash: "readable-hash",
            textCaptureMode: "full_readable_text",
            privateWindow: true,
            timestamp: Date(timeIntervalSince1970: 0)
        )

        let context = try payload.asBrowserContext()

        XCTAssertEqual(context.textCaptureMode, "private_window_metadata_only")
        XCTAssertTrue(context.privateWindow)
        XCTAssertEqual(context.schemaVersion, 1)
        XCTAssertEqual(context.captureID, "capture-1")
        XCTAssertEqual(context.sourceQuality, .extensionPrivate)
        XCTAssertNil(context.visibleText)
        XCTAssertNil(context.readableText)
        XCTAssertEqual(context.visibleTextHash, "visible-hash")
        XCTAssertEqual(context.readableTextHash, "readable-hash")
    }

    func testBrowserExtensionPayloadNormalizesWhitespaceAndEmptyFields() throws {
        let payload = BrowserExtensionUpdateRequest(
            schemaVersion: 1,
            captureID: "  capture-2  ",
            browser: "  Google Chrome  ",
            bundleIdentifier: "  com.google.Chrome  ",
            url: "  https://example.com/article  ",
            title: "  Example Article  ",
            activity: nil,
            pageID: "   ",
            mediaID: nil,
            mediaIsPlaying: nil,
            scrollPercent: 0,
            viewportSignature: "  viewport-2  ",
            noveltyScore: 1,
            visibleText: "  Visible article text  ",
            selectedText: "   ",
            readableText: nil,
            visibleTextHash: "  visible-hash  ",
            readableTextHash: nil,
            textCaptureMode: "  visible_viewport  ",
            privateWindow: false,
            timestamp: Date(timeIntervalSince1970: 0)
        )

        let context = try payload.asBrowserContext()

        XCTAssertEqual(context.captureID, "capture-2")
        XCTAssertEqual(context.browser, "Google Chrome")
        XCTAssertEqual(context.bundleIdentifier, "com.google.Chrome")
        XCTAssertEqual(context.url, "https://example.com/article")
        XCTAssertEqual(context.title, "Example Article")
        XCTAssertEqual(context.pageID, "example.com/article")
        XCTAssertEqual(context.viewportSignature, "viewport-2")
        XCTAssertEqual(context.visibleText, "Visible article text")
        XCTAssertNil(context.selectedText)
        XCTAssertEqual(context.visibleTextHash, "visible-hash")
        XCTAssertEqual(context.textCaptureMode, "visible_viewport")
    }

    func testBrowserExtensionPayloadRejectsBlankBrowserName() {
        let payload = BrowserExtensionUpdateRequest(
            schemaVersion: 1,
            captureID: "capture-blank-browser",
            browser: "   ",
            bundleIdentifier: "com.google.Chrome",
            url: "https://example.com/article",
            title: "Example Article",
            activity: .browsing,
            pageID: nil,
            mediaID: nil,
            mediaIsPlaying: nil,
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
            timestamp: Date(timeIntervalSince1970: 0)
        )

        XCTAssertThrowsError(try payload.asBrowserContext())
    }

    func testBrowserExtensionPayloadDropsSensitiveMetadataOnlyText() throws {
        let payload = BrowserExtensionUpdateRequest(
            schemaVersion: 1,
            captureID: "capture-sensitive",
            browser: "Google Chrome",
            bundleIdentifier: "com.google.Chrome",
            url: "https://example.com/account",
            title: "Account",
            activity: .browsing,
            pageID: nil,
            mediaID: nil,
            mediaIsPlaying: nil,
            scrollPercent: nil,
            viewportSignature: nil,
            noveltyScore: nil,
            visibleText: "Sensitive visible text",
            selectedText: "Sensitive selection",
            readableText: "Sensitive readable text",
            visibleTextHash: "visible-hash",
            readableTextHash: "readable-hash",
            textCaptureMode: "sensitive_metadata_only",
            privateWindow: false,
            timestamp: Date(timeIntervalSince1970: 0)
        )

        let context = try payload.asBrowserContext()

        XCTAssertNil(context.visibleText)
        XCTAssertNil(context.selectedText)
        XCTAssertNil(context.readableText)
        XCTAssertEqual(context.visibleTextHash, "visible-hash")
        XCTAssertEqual(context.readableTextHash, "readable-hash")
        XCTAssertEqual(context.sourceQuality, .extensionMetadataOnly)
    }

    func testBrowserExtensionPayloadClampsFarFutureTimestamp() throws {
        let payload = BrowserExtensionUpdateRequest(
            schemaVersion: 1,
            captureID: "capture-future",
            browser: "Google Chrome",
            bundleIdentifier: "com.google.Chrome",
            url: "https://example.com/article",
            title: "Example Article",
            activity: .browsing,
            pageID: nil,
            mediaID: nil,
            mediaIsPlaying: nil,
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
            timestamp: Date().addingTimeInterval(600)
        )

        let before = Date()
        let context = try payload.asBrowserContext()
        let after = Date()

        XCTAssertGreaterThanOrEqual(context.timestamp, before)
        XCTAssertLessThanOrEqual(context.timestamp, after)
    }

    func testBrowserExtensionPayloadRejectsUnsupportedSchemaVersion() throws {
        let payload = BrowserExtensionUpdateRequest(
            schemaVersion: 999,
            captureID: "capture-unsupported",
            browser: "Google Chrome",
            bundleIdentifier: "com.google.Chrome",
            url: "https://example.com/article",
            title: "Example Article",
            activity: .browsing,
            pageID: nil,
            mediaID: nil,
            mediaIsPlaying: nil,
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
            timestamp: Date(timeIntervalSince1970: 0)
        )

        XCTAssertThrowsError(try payload.asBrowserContext())
    }

    func testPermissionStateResolverDoesNotPersistScreenRecordingPendingRestart() {
        XCTAssertEqual(
            PermissionStateResolver.screenRecordingState(isGranted: false, requestedThisSession: true),
            .pendingRestart
        )
        XCTAssertEqual(
            PermissionStateResolver.screenRecordingState(isGranted: false, requestedThisSession: false),
            .notGranted
        )
        XCTAssertEqual(
            PermissionStateResolver.screenRecordingState(isGranted: true, requestedThisSession: true),
            .granted
        )

        let persisted = PermissionStateResolver.persistedRequestedKinds(
            from: ["screenRecording", "accessibility", "microphone", "unknown"]
        )

        XCTAssertFalse(persisted.contains(.screenRecording))
        XCTAssertEqual(persisted, [.accessibility, .microphone])
    }

    func testPermissionGuidancePrioritizesIdentityWarningAndRestartRecovery() {
        let notGranted = AppPermissionStatus(kind: .screenRecording, state: .notGranted)
        let pendingRestart = AppPermissionStatus(kind: .screenRecording, state: .pendingRestart)
        let granted = AppPermissionStatus(kind: .screenRecording, state: .granted)

        XCTAssertNil(
            PermissionGuidance.message(
                requiredStatuses: [granted],
                appIdentityWarning: "Move AuraBot.app to Applications."
            )
        )
        XCTAssertEqual(
            PermissionGuidance.message(
                requiredStatuses: [notGranted],
                appIdentityWarning: "Move AuraBot.app to Applications."
            ),
            "Move AuraBot.app to Applications."
        )
        XCTAssertTrue(
            PermissionGuidance.message(
                requiredStatuses: [pendingRestart],
                appIdentityWarning: nil
            )?.contains("restart Aura") == true
        )
        XCTAssertTrue(
            PermissionGuidance.message(
                requiredStatuses: [notGranted],
                appIdentityWarning: nil
            )?.contains("browser/app metadata") == true
        )
    }

    func testPermissionGuidanceRowCopyMatchesRequiredOptionalAndRestartStates() {
        let requiredMissing = AppPermissionStatus(kind: .screenRecording, state: .notGranted)
        let optionalMissing = AppPermissionStatus(kind: .microphone, state: .notGranted)
        let pendingRestart = AppPermissionStatus(kind: .screenRecording, state: .pendingRestart)
        let granted = AppPermissionStatus(kind: .accessibility, state: .granted)

        XCTAssertEqual(PermissionGuidance.actionTitle(for: requiredMissing), "Open Screen Recording")
        XCTAssertEqual(PermissionGuidance.actionTitle(for: pendingRestart), "Restart Aura")
        XCTAssertEqual(PermissionGuidance.actionTitle(for: granted), "Enabled")
        XCTAssertTrue(PermissionGuidance.detail(for: requiredMissing).contains("Required"))
        XCTAssertTrue(PermissionGuidance.detail(for: optionalMissing).contains("Optional"))
        XCTAssertTrue(PermissionGuidance.detail(for: pendingRestart).contains("relaunch"))
    }

    func testBrowserContextServiceReportsFreshExtensionStatus() async throws {
        var config = ExtensionConfig()
        config.freshnessSeconds = 30
        let service = BrowserContextService(config: config)
        let context = makeBrowserContext(
            source: .extensionData,
            browser: "Google Chrome",
            bundleIdentifier: "com.google.Chrome",
            url: "https://example.com/docs",
            title: "Example Docs",
            timestamp: Date()
        )

        await service.updateExtensionContext(context)
        let status = await service.currentContextStatus()

        XCTAssertTrue(status.hasFreshExtensionContext)
        XCTAssertNil(status.staleExtensionContext)
        XCTAssertNil(status.reason)
        XCTAssertEqual(status.context?.sourceQuality, .extensionFull)
    }

    func testBrowserContextServicePreservesStaleExtensionDiagnostics() async throws {
        var config = ExtensionConfig()
        config.freshnessSeconds = 1
        let service = BrowserContextService(config: config)
        let context = makeBrowserContext(
            source: .extensionData,
            browser: "Google Chrome",
            bundleIdentifier: "com.google.Chrome",
            url: "https://example.com/docs",
            title: "Example Docs",
            timestamp: Date(timeIntervalSince1970: 0)
        )

        await service.updateExtensionContext(context)
        let status = await service.currentContextStatus()

        XCTAssertEqual(status.staleExtensionContext?.captureID, "capture-fixture")
        XCTAssertEqual(status.reason, .extensionStale)
    }

    func testComputerUseConfigDecodesMissingPayloadWithAuraDefaults() throws {
        let payload = """
        {}
        """.data(using: .utf8)!

        let config = try JSONDecoder().decode(AppConfig.self, from: payload)

        XCTAssertFalse(config.computerUse.enabled)
        XCTAssertFalse(config.computerUse.recordTrajectories)
        XCTAssertEqual(config.computerUse.captureMode, .som)
        XCTAssertEqual(config.computerUse.maxImageDimension, 1600)
    }

    func testComputerUseConfigIgnoresRemovedHelperSettings() throws {
        let payload = """
        {
          "computerUse": {
            "enabled": true,
            "autoStart": false,
            "allowUpdateChecks": false,
            "installedVersion": "0.1.2",
            "recordTrajectories": true,
            "captureMode": "vision",
            "maxImageDimension": 1024
          }
        }
        """.data(using: .utf8)!

        let config = try JSONDecoder().decode(AppConfig.self, from: payload)

        XCTAssertTrue(config.computerUse.enabled)
        XCTAssertTrue(config.computerUse.recordTrajectories)
        XCTAssertEqual(config.computerUse.captureMode, .vision)
        XCTAssertEqual(config.computerUse.maxImageDimension, 1024)
    }

    func testComputerUseConfigRejectsInvalidCaptureMode() {
        let payload = """
        {
          "computerUse": {
            "captureMode": "screeenshot"
          }
        }
        """.data(using: .utf8)!

        XCTAssertThrowsError(try JSONDecoder().decode(AppConfig.self, from: payload))
    }

    func testAppConfigSanitizerClampsRangesAndRestoresRequiredDefaults() {
        var config = AppConfig.default
        config.capture.intervalSeconds = -10
        config.capture.quality = 500
        config.capture.maxWidth = 1
        config.capture.meaningfulChangeThreshold = 100
        config.llm.baseURL = "not a url"
        config.llm.model = "   "
        config.llm.openRouterChatModel = ""
        config.llm.openRouterAPIKey = "  secret  "
        config.llm.maxTokens = 1_000_000
        config.llm.temperature = -1
        config.memory.baseURL = "ftp://example.com"
        config.memory.userID = "   "
        config.memory.collectionName = ""
        config.app.memoryWindow = 0
        config.app.activePluginID = "   "
        config.browserExtension.port = 99_999
        config.browserExtension.freshnessSeconds = 0
        config.browserExtension.apiKey = "  ext-key  "
        config.browserExtension.allowedOrigins = ["  http://localhost:  ", "", "http://localhost:"]
        config.computerUse.maxImageDimension = 99_999

        let sanitized = config.sanitizedForPersistence()

        XCTAssertEqual(sanitized.capture.intervalSeconds, 10)
        XCTAssertEqual(sanitized.capture.quality, 100)
        XCTAssertEqual(sanitized.capture.maxWidth, 320)
        XCTAssertEqual(sanitized.capture.meaningfulChangeThreshold, 64)
        XCTAssertEqual(sanitized.llm.baseURL, LLMConfig().baseURL)
        XCTAssertEqual(sanitized.llm.model, LLMConfig().model)
        XCTAssertEqual(sanitized.llm.openRouterChatModel, LLMConfig().openRouterChatModel)
        XCTAssertEqual(sanitized.llm.openRouterAPIKey, "secret")
        XCTAssertEqual(sanitized.llm.maxTokens, 64_000)
        XCTAssertEqual(sanitized.llm.temperature, 0)
        XCTAssertEqual(sanitized.memory.baseURL, MemoryConfig.managedPgliteBaseURL)
        XCTAssertEqual(sanitized.memory.userID, "default_user")
        XCTAssertEqual(sanitized.memory.collectionName, "screen_memories_v3")
        XCTAssertEqual(sanitized.app.memoryWindow, 1)
        XCTAssertNil(sanitized.app.activePluginID)
        XCTAssertEqual(sanitized.browserExtension.port, 65_535)
        XCTAssertEqual(sanitized.browserExtension.freshnessSeconds, 1)
        XCTAssertEqual(sanitized.browserExtension.apiKey, "ext-key")
        XCTAssertEqual(sanitized.browserExtension.allowedOrigins, ["http://localhost:"])
        XCTAssertEqual(sanitized.computerUse.maxImageDimension, 4096)
    }

    func testAppConfigLoadSanitizesMalformedPersistedConfiguration() throws {
        let outputURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("aurabot-config-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: outputURL) }
        let payload = """
        {
          "capture": {
            "intervalSeconds": 1,
            "quality": 5
          },
          "llm": {
            "baseURL": "nope",
            "model": ""
          },
          "memory": {
            "baseURL": "http://localhost:8000",
            "userID": "   "
          },
          "extension": {
            "port": -1,
            "freshnessSeconds": -2,
            "allowedOrigins": []
          },
          "computerUse": {
            "maxImageDimension": -50
          }
        }
        """
        try payload.data(using: .utf8)!.write(to: outputURL)

        let loaded = AppConfig.load(from: outputURL.path)

        XCTAssertEqual(loaded.capture.intervalSeconds, 10)
        XCTAssertEqual(loaded.capture.quality, 30)
        XCTAssertEqual(loaded.llm.baseURL, LLMConfig().baseURL)
        XCTAssertEqual(loaded.llm.model, LLMConfig().model)
        XCTAssertEqual(loaded.memory.baseURL, MemoryConfig.managedPgliteBaseURL)
        XCTAssertEqual(loaded.memory.userID, "default_user")
        XCTAssertEqual(loaded.browserExtension.port, 1)
        XCTAssertEqual(loaded.browserExtension.freshnessSeconds, 1)
        XCTAssertEqual(loaded.browserExtension.allowedOrigins, ExtensionConfig().allowedOrigins)
        XCTAssertEqual(loaded.computerUse.maxImageDimension, 0)
    }

    func testComputerUsePermissionParsingFromEmbeddedToolText() async throws {
        var config = ComputerUseConfig()
        config.enabled = true
        let service = ComputerUseService(
            config: config,
            tools: FakeComputerUseTools(
                results: [
                    "check_permissions": .success(
                        output: """
                        Accessibility: granted
                        Screen Recording: NOT granted
                        """
                    ),
                ]
            )
        )

        let permissions = try await service.checkPermissions(prompt: false)

        XCTAssertTrue(permissions.accessibility)
        XCTAssertFalse(permissions.screenRecording)
    }

    func testComputerUsePermissionParserTreatsNegativePhrasesAsDenied() {
        XCTAssertFalse(
            ComputerUsePermissionParser.permissionLineValue(
                label: "Screen Recording",
                in: "Screen Recording: not authorized"
            )
        )
        XCTAssertFalse(
            ComputerUsePermissionParser.permissionLineValue(
                label: "Accessibility",
                in: "Accessibility: not allowed"
            )
        )
        XCTAssertTrue(
            ComputerUsePermissionParser.permissionLineValue(
                label: "Accessibility",
                in: "Accessibility: authorized"
            )
        )
    }

    func testComputerUsePermissionParserNormalizesStructuredValues() {
        XCTAssertTrue(ComputerUsePermissionParser.boolValue(.string("  Authorized  ")))
        XCTAssertTrue(ComputerUsePermissionParser.boolValue(.string("YES")))
        XCTAssertFalse(ComputerUsePermissionParser.boolValue(.string(" not granted ")))
        XCTAssertFalse(ComputerUsePermissionParser.boolValue(.string("disabled")))
        XCTAssertTrue(ComputerUsePermissionParser.boolValue(.int(1)))
        XCTAssertFalse(ComputerUsePermissionParser.boolValue(.double(0)))
    }

    func testComputerUsePermissionGuidanceNamesMissingPermission() {
        XCTAssertEqual(
            ComputerUsePermissionParser.guidanceMessage(
                for: ComputerUsePermissionStatus(accessibility: false, screenRecording: true)
            ),
            "AuraBot needs Accessibility permission to inspect and control app windows."
        )
        XCTAssertEqual(
            ComputerUsePermissionParser.guidanceMessage(
                for: ComputerUsePermissionStatus(accessibility: true, screenRecording: false)
            ),
            "AuraBot needs Screen Recording permission to capture window state for Computer Use."
        )
        XCTAssertEqual(
            ComputerUsePermissionParser.guidanceMessage(
                for: ComputerUsePermissionStatus(accessibility: true, screenRecording: true)
            ),
            "Computer Use is ready."
        )
    }

    func testComputerUsePermissionFailureSurfacesAsFailedStatus() async throws {
        var config = ComputerUseConfig()
        config.enabled = true
        let service = ComputerUseService(
            config: config,
            tools: FakeComputerUseTools(
                results: [
                    "set_config": .success(),
                    "check_permissions": .failure("Unknown tool: check_permissions"),
                ]
            )
        )

        let status = await service.refreshStatus()

        XCTAssertEqual(status.state, .failed)
        XCTAssertEqual(status.message, "Unknown tool: check_permissions")
    }

    func testComputerUseRoutesListAppsAndWindowsThroughEmbeddedTools() async throws {
        let fake = FakeComputerUseTools(
            results: [
                "list_apps": .success(output: "[{\"name\":\"Finder\"}]"),
                "list_windows": .success(output: "[{\"title\":\"Desktop\"}]"),
            ]
        )
        let service = ComputerUseService(config: ComputerUseConfig(), tools: fake)

        let apps = await service.listApps()
        let windows = await service.listWindows()

        XCTAssertTrue(apps.succeeded)
        XCTAssertEqual(apps.output, "[{\"name\":\"Finder\"}]")
        XCTAssertTrue(windows.succeeded)
        XCTAssertEqual(windows.output, "[{\"title\":\"Desktop\"}]")
        let callNames = await fake.calls.map(\.name)
        XCTAssertEqual(callNames, ["list_apps", "list_windows"])
    }

    func testComputerUseSmokeTestSuccessAndFailureStates() async throws {
        var config = ComputerUseConfig()
        config.enabled = true
        let successService = ComputerUseService(
            config: config,
            tools: FakeComputerUseTools(
                results: [
                    "set_config": .success(),
                    "check_permissions": .success(
                        structuredContent: .object([
                            "accessibility": .bool(true),
                            "screenRecording": .bool(true),
                        ])
                    ),
                    "list_windows": .success(output: "[]"),
                ]
            )
        )
        let failingService = ComputerUseService(
            config: config,
            tools: FakeComputerUseTools(
                results: [
                    "set_config": .success(),
                    "check_permissions": .success(
                        structuredContent: .object([
                            "accessibility": .bool(true),
                            "screenRecording": .bool(true),
                        ])
                    ),
                    "list_windows": .failure("AuraBot Computer Use could not list windows."),
                ]
            )
        )

        let success = await successService.runSafeSmokeTest()
        let failure = await failingService.runSafeSmokeTest()

        XCTAssertEqual(success.state, .ready)
        XCTAssertEqual(success.message, "Computer Use test passed.")
        XCTAssertEqual(failure.state, .failed)
    }

    func testComputerUseScreenshotWritesEmbeddedImageData() async throws {
        let outputURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("aurabot-computer-use-\(UUID().uuidString).png")
        defer { try? FileManager.default.removeItem(at: outputURL) }
        let imageData = Data([0x89, 0x50, 0x4E, 0x47])
        let service = ComputerUseService(
            config: ComputerUseConfig(),
            tools: FakeComputerUseTools(
                results: [
                    "set_config": .success(),
                    "screenshot": .success(output: "captured", imageData: imageData),
                ]
            )
        )

        let result = await service.screenshot(windowID: 42, outputURL: outputURL)

        XCTAssertTrue(result.succeeded)
        XCTAssertEqual(result.output, outputURL.path)
        XCTAssertEqual(try Data(contentsOf: outputURL), imageData)
    }

    func testOldComputerUseImplementationIsRemoved() {
        let repoRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()

        XCTAssertFalse(
            FileManager.default.fileExists(
                atPath: repoRoot
                    .appendingPathComponent("apps/macos/Sources/AuraBot/ComputerUse")
                    .path
            )
        )
        XCTAssertFalse(
            FileManager.default.fileExists(
                atPath: repoRoot
                    .appendingPathComponent("apps/macos/Sources/AuraBot/Resources/ComputerUseSkills")
                    .path
            )
        )
    }

    private func makeBrowserContext(
        source: BrowserContextSource,
        browser: String,
        bundleIdentifier: String,
        url: String,
        title: String,
        visibleTextHash: String? = "visible-hash",
        readableTextHash: String? = nil,
        textCaptureMode: String? = "visible_viewport",
        timestamp: Date = Date(timeIntervalSince1970: 0)
    ) -> BrowserContext {
        let derived = BrowserContextService.deriveActivity(url: url, title: title)

        return BrowserContext(
            source: source,
            browser: browser,
            bundleIdentifier: bundleIdentifier,
            url: url,
            title: title,
            activity: derived.activity,
            pageID: derived.pageID,
            mediaID: derived.mediaID,
            mediaIsPlaying: derived.activity == .media,
            scrollPercent: 42,
            viewportSignature: "viewport-1",
            noveltyScore: 0.2,
            visibleText: "Visible documentation text",
            selectedText: nil,
            readableText: nil,
            visibleTextHash: visibleTextHash,
            readableTextHash: readableTextHash,
            textCaptureMode: textCaptureMode,
            privateWindow: false,
            captureID: "capture-fixture",
            timestamp: timestamp
        )
    }

}

@available(macOS 14.0, *)
private struct FakeComputerUseToolCall: Equatable, Sendable {
    let name: String
    let arguments: [String: ComputerUseArgument]
}

@available(macOS 14.0, *)
private actor FakeComputerUseTools: ComputerUseToolCalling {
    nonisolated let toolNames: [String]
    private let results: [String: ComputerUseToolResult]
    private var recordedCalls: [FakeComputerUseToolCall] = []

    init(results: [String: ComputerUseToolResult]) {
        self.results = results
        self.toolNames = Array(results.keys).sorted()
    }

    var calls: [FakeComputerUseToolCall] {
        recordedCalls
    }

    func call(
        _ name: String,
        arguments: [String: ComputerUseArgument]
    ) async throws -> ComputerUseToolResult {
        recordedCalls.append(FakeComputerUseToolCall(name: name, arguments: arguments))
        return results[name] ?? .failure("No fake result for \(name).")
    }
}

@available(macOS 14.0, *)
private func makeWorkspacePluginDescriptor() -> WorkspacePluginDescriptor {
    WorkspacePluginDescriptor(
        pluginID: "com.aurabot.ai-tutor",
        name: "AI Tutor",
        takeover: PluginTakeoverPolicy(
            ui: .replace,
            agent: .replace,
            context: .replace,
            capture: .replace,
            memory: .augment,
            retrieval: .replace,
            window: .replace,
            commands: .replace,
            settings: .augment
        ),
        appBehavior: AppBehaviorPolicy(
            navigation: .pluginWorkspace,
            commands: .pluginWorkspace,
            fallback: .hostDefault
        ),
        capturePolicy: CapturePolicy(
            priority: [.browserDOM, .browserTranscript, .appMetadata],
            fallback: .none,
            redaction: .strict
        ),
        windowPolicy: WindowPolicy(
            presentation: .floatingOverlay,
            level: .alwaysOnTop,
            hideWhen: [.screenSharing, .sensitiveApp],
            excludePluginUIFromCapture: true
        )
    )
}

@available(macOS 14.0, *)
private func decodeMemoryFixture<T: Decodable>(_ name: String, as type: T.Type) throws -> T {
    let repoRoot = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
    let url = repoRoot
        .appendingPathComponent("services/memory-pglite/src/test-fixtures")
        .appendingPathComponent("\(name).json")
    let data = try Data(contentsOf: url)
    return try MemoryV2JSON.makeDecoder().decode(type, from: data)
}
