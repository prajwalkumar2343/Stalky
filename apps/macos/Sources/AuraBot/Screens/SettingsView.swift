import SwiftUI
import AppKit

@available(macOS 14.0, *)
struct SettingsView: View {
    @ObservedObject var service: AppService
    @State private var captureEnabled = true
    @State private var captureInterval: Double = 30
    @State private var captureQuality: Double = 60
    @State private var llmURL = "https://openrouter.ai/api/v1"
    @State private var openRouterAPIKey = ""
    @State private var visionModel = ""
    @State private var chatModel = ""
    @State private var contextCollectorRewriteEnabled = false
    @State private var memoryURL = MemoryConfig.managedPgliteBaseURL
    @State private var memoryAPIKey = ""
    @State private var memoryUserID = "default_user"
    @State private var memoryCollection = "screen_memories_v3"
    @State private var browserExtensionAPIKey = ""
    @State private var computerUseEnabled = false
    @State private var computerUseRecordTrajectories = false
    @State private var computerUseCaptureMode: ComputerUseCaptureMode = .som
    @State private var computerUseMaxImageDimension: Double = 1600
    @State private var overlayPosition: OverlayPosition = .bottomRight
    @State private var overlayShowsOverFullScreenApps = true
    @State private var showSavedToast = false
    @State private var appearAnimation = false
    @State private var isSaving = false
    
    var body: some View {
        ScrollView(showsIndicators: false) {
            VStack(alignment: .leading, spacing: Spacing.xxxl) {
                // Header
                SettingsHeader()
                    .opacity(appearAnimation ? 1 : 0)
                    .offset(y: appearAnimation ? 0 : 20)
                
                // Capture Settings
                CaptureSettingsSection(
                    captureEnabled: $captureEnabled,
                    captureInterval: $captureInterval,
                    captureQuality: $captureQuality,
                    overlayPosition: $overlayPosition,
                    overlayShowsOverFullScreenApps: $overlayShowsOverFullScreenApps
                )
                .opacity(appearAnimation ? 1 : 0)
                .offset(y: appearAnimation ? 0 : 20)
                
                // AI Settings
                AISettingsSection(
                    llmURL: $llmURL,
                    openRouterAPIKey: $openRouterAPIKey,
                    visionModel: $visionModel,
                    chatModel: $chatModel,
                    contextCollectorRewriteEnabled: $contextCollectorRewriteEnabled,
                    memoryURL: $memoryURL,
                    memoryAPIKey: $memoryAPIKey,
                    memoryUserID: $memoryUserID,
                    memoryCollection: $memoryCollection,
                    browserExtensionAPIKey: $browserExtensionAPIKey
                )
                .opacity(appearAnimation ? 1 : 0)
                .offset(y: appearAnimation ? 0 : 20)

                ComputerUseSettingsSection(
                    service: service,
                    enabled: $computerUseEnabled,
                    recordTrajectories: $computerUseRecordTrajectories,
                    captureMode: $computerUseCaptureMode,
                    maxImageDimension: $computerUseMaxImageDimension
                )
                .opacity(appearAnimation ? 1 : 0)
                .offset(y: appearAnimation ? 0 : 20)
                
                // Permissions
                PermissionsSection(service: service)
                    .opacity(appearAnimation ? 1 : 0)
                    .offset(y: appearAnimation ? 0 : 20)

                // Save button
                SaveSection(
                    isSaving: $isSaving,
                    onSave: saveSettings
                )
                .opacity(appearAnimation ? 1 : 0)
                .offset(y: appearAnimation ? 0 : 20)
            }
            .padding(Spacing.xxxl)
        }
        .background(Color.clear)
        .overlay(
            Group {
                if showSavedToast {
                    SavedToast()
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
            }
        )
        .onAppear {
            loadFromService()
            service.refreshPermissionStatuses()
            withAnimation(.spring(response: 0.6, dampingFraction: 0.8).delay(0.1)) {
                appearAnimation = true
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            service.refreshPermissionStatuses()
        }
    }

    private func loadFromService() {
        let config = service.config

        captureEnabled = config.capture.enabled
        captureInterval = Double(config.capture.intervalSeconds)
        captureQuality = Double(config.capture.quality)
        llmURL = config.llm.baseURL
        openRouterAPIKey = config.llm.openRouterAPIKey
        visionModel = config.llm.model
        chatModel = config.llm.openRouterChatModel
        contextCollectorRewriteEnabled = config.llm.contextCollectorRewrite.enabled
        memoryURL = config.memory.baseURL
        memoryAPIKey = config.memory.apiKey
        memoryUserID = config.memory.userID
        memoryCollection = config.memory.collectionName
        browserExtensionAPIKey = config.browserExtension.apiKey
        computerUseEnabled = config.computerUse.enabled
        computerUseRecordTrajectories = config.computerUse.recordTrajectories
        computerUseCaptureMode = config.computerUse.captureMode
        computerUseMaxImageDimension = Double(config.computerUse.maxImageDimension)
        overlayPosition = config.app.overlayPosition
        overlayShowsOverFullScreenApps = config.app.overlayShowsOverFullScreenApps
    }

    private func saveSettings() {
        isSaving = true

        Task {
            var updatedConfig = service.config
            updatedConfig.capture.enabled = captureEnabled
            updatedConfig.capture.intervalSeconds = Int(captureInterval)
            updatedConfig.capture.quality = Int(captureQuality)
            updatedConfig.llm.baseURL = llmURL.trimmingCharacters(in: .whitespacesAndNewlines)
            updatedConfig.llm.openRouterAPIKey = openRouterAPIKey.trimmingCharacters(in: .whitespacesAndNewlines)
            updatedConfig.llm.model = visionModel.trimmingCharacters(in: .whitespacesAndNewlines)
            updatedConfig.llm.openRouterChatModel = chatModel.trimmingCharacters(in: .whitespacesAndNewlines)
            updatedConfig.llm.contextCollectorRewrite.enabled = contextCollectorRewriteEnabled
            updatedConfig.memory.baseURL = memoryURL.trimmingCharacters(in: .whitespacesAndNewlines)
            updatedConfig.memory.apiKey = memoryAPIKey.trimmingCharacters(in: .whitespacesAndNewlines)
            updatedConfig.memory.userID = memoryUserID.trimmingCharacters(in: .whitespacesAndNewlines)
            updatedConfig.memory.collectionName = memoryCollection.trimmingCharacters(in: .whitespacesAndNewlines)
            updatedConfig.browserExtension.apiKey = browserExtensionAPIKey.trimmingCharacters(in: .whitespacesAndNewlines)
            updatedConfig.computerUse.enabled = computerUseEnabled
            updatedConfig.computerUse.recordTrajectories = computerUseRecordTrajectories
            updatedConfig.computerUse.captureMode = computerUseCaptureMode
            updatedConfig.computerUse.maxImageDimension = Int(computerUseMaxImageDimension)
            if service.config.app.overlayPosition != overlayPosition {
                updatedConfig.app.overlayOrigin = nil
            }
            updatedConfig.app.overlayPosition = overlayPosition
            updatedConfig.app.overlayShowsOverFullScreenApps = overlayShowsOverFullScreenApps

            do {
                try await service.saveConfiguration(updatedConfig)

                await MainActor.run {
                    isSaving = false
                    withAnimation {
                        showSavedToast = true
                    }

                    DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                        withAnimation {
                            showSavedToast = false
                        }
                    }
                }
            } catch {
                await MainActor.run {
                    isSaving = false
                }
                print("Failed to save configuration: \(error)")
            }
        }
    }
}

@available(macOS 14.0, *)
struct SettingsHeader: View {
    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.xs) {
            Text("Settings")
                .font(Typography.title1)
                .foregroundColor(Colors.textPrimary)
            
            Text("Customize your Aura experience")
                .font(Typography.body)
                .foregroundColor(Colors.textSecondary)
        }
    }
}

@available(macOS 14.0, *)
struct CaptureSettingsSection: View {
    @Binding var captureEnabled: Bool
    @Binding var captureInterval: Double
    @Binding var captureQuality: Double
    @Binding var overlayPosition: OverlayPosition
    @Binding var overlayShowsOverFullScreenApps: Bool
    
    var body: some View {
        SettingsSection(title: "Capture", icon: "camera.fill") {
            VStack(spacing: Spacing.xl) {
                OverlayPositionPicker(position: $overlayPosition)

                Divider()
                    .background(Colors.glassBorder)

                CustomToggle(
                    title: "Show Over Full-Screen Apps",
                    description: "Keep Aura visible above apps in full-screen spaces.",
                    isOn: $overlayShowsOverFullScreenApps
                )

                Divider()
                    .background(Colors.glassBorder)

                // Enable toggle
                CustomToggle(
                    title: "Screen Capture",
                    description: "Automatically capture and analyze your screen",
                    isOn: $captureEnabled
                )
                
                Divider()
                    .background(Colors.glassBorder)
                
                // Interval slider
                VStack(alignment: .leading, spacing: Spacing.md) {
                    HStack {
                        Text("Capture Interval")
                            .font(Typography.subheadline)
                            .foregroundColor(Colors.textPrimary)
                        
                        Spacer()
                        
                        Text("\(Int(captureInterval))s")
                            .font(Typography.subheadline)
                            .fontWeight(.semibold)
                            .foregroundColor(Colors.primary)
                            .padding(.horizontal, Spacing.sm)
                            .padding(.vertical, Spacing.xs)
                            .background(
                                ZStack {
                                    RoundedRectangle(cornerRadius: Radius.sm)
                                        .fill(Colors.primary.opacity(0.1))
                                    RoundedRectangle(cornerRadius: Radius.sm)
                                        .stroke(Colors.primary.opacity(0.15), lineWidth: 1)
                                }
                            )
                    }
                    
                    CustomSlider(
                        value: $captureInterval,
                        range: 10...300,
                        step: 5
                    )
                    
                    HStack {
                        Text("10s")
                        Spacer()
                        Text("5min")
                    }
                    .font(Typography.caption2)
                    .foregroundColor(Colors.textMuted)
                }
                
                Divider()
                    .background(Colors.glassBorder)
                
                // Quality slider
                VStack(alignment: .leading, spacing: Spacing.md) {
                    HStack {
                        Text("JPEG Quality")
                            .font(Typography.subheadline)
                            .foregroundColor(Colors.textPrimary)
                        
                        Spacer()
                        
                        Text("\(Int(captureQuality))%")
                            .font(Typography.subheadline)
                            .fontWeight(.semibold)
                            .foregroundColor(Colors.primary)
                            .padding(.horizontal, Spacing.sm)
                            .padding(.vertical, Spacing.xs)
                            .background(
                                ZStack {
                                    RoundedRectangle(cornerRadius: Radius.sm)
                                        .fill(Colors.primary.opacity(0.1))
                                    RoundedRectangle(cornerRadius: Radius.sm)
                                        .stroke(Colors.primary.opacity(0.15), lineWidth: 1)
                                }
                            )
                    }
                    
                    CustomSlider(
                        value: $captureQuality,
                        range: 30...100,
                        step: 5
                    )
                    
                    HStack {
                        Text("Low")
                        Spacer()
                        Text("High")
                    }
                    .font(Typography.caption2)
                    .foregroundColor(Colors.textMuted)
                }
            }
        }
    }
}

@available(macOS 14.0, *)
struct OverlayPositionPicker: View {
    @Binding var position: OverlayPosition

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            HStack(spacing: Spacing.md) {
                Image(systemName: "rectangle.on.rectangle")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(Colors.primary)

                VStack(alignment: .leading, spacing: Spacing.xs) {
                    Text("Overlay Position")
                        .font(Typography.subheadline)
                        .fontWeight(.medium)
                        .foregroundColor(Colors.textPrimary)

                    Text("Choose a snap point. Drag Aura on screen for a custom spot.")
                        .font(Typography.caption)
                        .foregroundColor(Colors.textMuted)
                }

                Spacer()
            }

            Picker("Overlay Position", selection: $position) {
                ForEach(OverlayPosition.allCases) { option in
                    Text(option.displayName).tag(option)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
        }
    }
}

@available(macOS 14.0, *)
struct AISettingsSection: View {
    @Binding var llmURL: String
    @Binding var openRouterAPIKey: String
    @Binding var visionModel: String
    @Binding var chatModel: String
    @Binding var contextCollectorRewriteEnabled: Bool
    @Binding var memoryURL: String
    @Binding var memoryAPIKey: String
    @Binding var memoryUserID: String
    @Binding var memoryCollection: String
    @Binding var browserExtensionAPIKey: String
    
    var body: some View {
        SettingsSection(title: "AI & Memory", icon: "brain.head.profile") {
            VStack(spacing: Spacing.xl) {
                CustomTextField(
                    title: "LLM Base URL",
                    placeholder: "https://openrouter.ai/api/v1",
                    text: $llmURL,
                    icon: "network"
                )
                
                Divider()
                    .background(Colors.glassBorder)

                CustomTextField(
                    title: "OpenRouter API Key",
                    placeholder: "sk-or-v1-...",
                    text: $openRouterAPIKey,
                    icon: "key.horizontal",
                    isSecure: true
                )

                Divider()
                    .background(Colors.glassBorder)

                CustomTextField(
                    title: "Vision Model",
                    placeholder: "google/gemini-2.5-flash-preview",
                    text: $visionModel,
                    icon: "photo.on.rectangle"
                )

                Divider()
                    .background(Colors.glassBorder)

                CustomTextField(
                    title: "Chat Model",
                    placeholder: "anthropic/claude-3.5-sonnet",
                    text: $chatModel,
                    icon: "text.bubble"
                )

                Divider()
                    .background(Colors.glassBorder)

                VStack(alignment: .leading, spacing: Spacing.md) {
                    CustomToggle(
                        title: "Optional Context Collector Rewrites",
                        description: "Allow OpenRouter to rewrite app-specific context collection code only when the selected chat model meets the configured capability threshold.",
                        isOn: $contextCollectorRewriteEnabled
                    )

                    let rules = ContextCollectorRewriteModelRule.defaultRules.map(\.label).joined(separator: ", ")
                    let eligible = LLMConfig(
                        baseURL: llmURL,
                        model: visionModel,
                        maxTokens: 512,
                        temperature: 0.7,
                        timeoutSeconds: 30,
                        openRouterAPIKey: openRouterAPIKey,
                        openRouterChatModel: chatModel,
                        contextCollectorRewrite: ContextCollectorRewritePolicy(
                            enabled: contextCollectorRewriteEnabled,
                            allowedModels: ContextCollectorRewriteModelRule.defaultRules
                        )
                    ).allowsContextCollectorRewrite()

                    Text("Allowlist: \(rules)")
                        .font(Typography.caption)
                        .foregroundColor(Colors.textMuted)

                    Text(eligible ? "Current chat model is eligible." : "Current chat model is not eligible.")
                        .font(Typography.caption)
                        .foregroundColor(eligible ? Colors.success : Colors.textMuted)
                }

                Divider()
                    .background(Colors.glassBorder)
                
                CustomTextField(
                    title: "Memory API URL",
                    placeholder: MemoryConfig.managedPgliteBaseURL,
                    text: $memoryURL,
                    icon: "server.rack"
                )

                Divider()
                    .background(Colors.glassBorder)

                CustomTextField(
                    title: "Memory API Key",
                    placeholder: "Optional bearer token",
                    text: $memoryAPIKey,
                    icon: "lock.shield",
                    isSecure: true
                )

                Divider()
                    .background(Colors.glassBorder)

                CustomTextField(
                    title: "Browser Extension API Key",
                    placeholder: "Required for extension context updates",
                    text: $browserExtensionAPIKey,
                    icon: "puzzlepiece.extension",
                    isSecure: true
                )

                Divider()
                    .background(Colors.glassBorder)

                CustomTextField(
                    title: "Memory User ID",
                    placeholder: "default_user",
                    text: $memoryUserID,
                    icon: "person"
                )

                Divider()
                    .background(Colors.glassBorder)

                CustomTextField(
                    title: "Collection Name",
                    placeholder: "screen_memories_v3",
                    text: $memoryCollection,
                    icon: "shippingbox"
                )
            }
        }
    }
}

struct PermissionsSection: View {
    @ObservedObject var service: AppService

    var body: some View {
        SettingsSection(title: "Permissions", icon: "lock.shield") {
            VStack(alignment: .leading, spacing: Spacing.lg) {
                HStack {
                    VStack(alignment: .leading, spacing: Spacing.xs) {
                        Text("Live macOS privacy access")
                            .font(Typography.headline)
                            .foregroundColor(Colors.textPrimary)

                        Text("Click any checklist item to jump directly into the matching System Settings panel.")
                            .font(Typography.callout)
                            .foregroundColor(Colors.textSecondary)
                    }

                    Spacer()
                }

                PermissionChecklistGroup(statuses: service.permissionStatuses) { kind in
                    service.requestPermission(kind)
                }

                BrowserExtensionSetupCard(service: service)
            }
        }
    }
}

@available(macOS 14.0, *)
struct ComputerUseSettingsSection: View {
    @ObservedObject var service: AppService
    @Binding var enabled: Bool
    @Binding var recordTrajectories: Bool
    @Binding var captureMode: ComputerUseCaptureMode
    @Binding var maxImageDimension: Double

    var body: some View {
        SettingsSection(title: "Computer Use", icon: "cursorarrow.motionlines") {
            VStack(alignment: .leading, spacing: Spacing.xl) {
                HStack(alignment: .top, spacing: Spacing.lg) {
                    Image(systemName: service.computerUseStatus.state.symbolName)
                        .font(.system(size: 24, weight: .semibold))
                        .foregroundColor(service.computerUseStatus.state.tintColor)
                        .frame(width: 32, height: 32)

                    VStack(alignment: .leading, spacing: Spacing.xs) {
                        Text(service.computerUseStatus.state.displayName)
                            .font(Typography.headline)
                            .foregroundColor(Colors.textPrimary)

                        Text(service.computerUseStatus.message)
                            .font(Typography.callout)
                            .foregroundColor(Colors.textSecondary)
                    }

                    Spacer()
                }

                Divider()
                    .background(Colors.glassBorder)

                CustomToggle(
                    title: "Enable Computer Use",
                    description: "Let AuraBot inspect and control app windows when you ask it to.",
                    isOn: $enabled
                )

                Divider()
                    .background(Colors.glassBorder)

                VStack(alignment: .leading, spacing: Spacing.md) {
                    HStack(spacing: Spacing.md) {
                        Image(systemName: "square.stack.3d.down.right")
                            .font(.system(size: 16, weight: .semibold))
                            .foregroundColor(Colors.primary)

                        VStack(alignment: .leading, spacing: Spacing.xs) {
                            Text("Capture Mode")
                                .font(Typography.subheadline)
                                .fontWeight(.medium)
                                .foregroundColor(Colors.textPrimary)

                            Text(captureMode.description)
                                .font(Typography.caption)
                                .foregroundColor(Colors.textMuted)
                        }

                        Spacer()
                    }

                    Picker("Capture Mode", selection: $captureMode) {
                        ForEach(ComputerUseCaptureMode.allCases) { mode in
                            Text(mode.displayName).tag(mode)
                        }
                    }
                    .pickerStyle(.segmented)
                }

                Divider()
                    .background(Colors.glassBorder)

                VStack(alignment: .leading, spacing: Spacing.md) {
                    HStack {
                        Text("Max Image Dimension")
                            .font(Typography.subheadline)
                            .foregroundColor(Colors.textPrimary)

                        Spacer()

                        Text(maxImageDimensionLabel)
                            .font(Typography.subheadline)
                            .fontWeight(.semibold)
                            .foregroundColor(Colors.primary)
                            .padding(.horizontal, Spacing.sm)
                            .padding(.vertical, Spacing.xs)
                            .background(
                                ZStack {
                                    RoundedRectangle(cornerRadius: Radius.sm)
                                        .fill(Colors.primary.opacity(0.1))
                                    RoundedRectangle(cornerRadius: Radius.sm)
                                        .stroke(Colors.primary.opacity(0.15), lineWidth: 1)
                                }
                            )
                    }

                    CustomSlider(
                        value: $maxImageDimension,
                        range: 0...4096,
                        step: 128
                    )

                    HStack {
                        Text("Native")
                        Spacer()
                        Text("4096 px")
                    }
                    .font(Typography.caption2)
                    .foregroundColor(Colors.textMuted)
                }

                Divider()
                    .background(Colors.glassBorder)

                CustomToggle(
                    title: "Record Trajectories",
                    description: "Save computer-use action traces for debugging and replay.",
                    isOn: $recordTrajectories
                )

                Divider()
                    .background(Colors.glassBorder)

                HStack(spacing: Spacing.md) {
                    SecondaryButton("Permissions", icon: "lock.shield") {
                        Task { await service.requestComputerUsePermissions() }
                    }

                    SecondaryButton("Run Test", icon: "checkmark.seal") {
                        Task { await service.runComputerUseSmokeTest() }
                    }
                }
            }
        }
    }

    private var maxImageDimensionLabel: String {
        let value = Int(maxImageDimension)
        return value == 0 ? "Native" : "\(value) px"
    }
}

@available(macOS 14.0, *)
private extension ComputerUseStatus.State {
    var displayName: String {
        switch self {
        case .disabled:
            return "Disabled"
        case .starting:
            return "Starting"
        case .ready:
            return "Ready"
        case .needsPermission:
            return "Needs Permission"
        case .failed:
            return "Needs Attention"
        }
    }

    var symbolName: String {
        switch self {
        case .ready:
            return "checkmark.circle.fill"
        case .needsPermission:
            return "lock.trianglebadge.exclamationmark.fill"
        case .starting:
            return "hourglass.circle.fill"
        case .failed:
            return "exclamationmark.triangle.fill"
        case .disabled:
            return "desktopcomputer"
        }
    }

    var tintColor: Color {
        switch self {
        case .ready:
            return Colors.success
        case .needsPermission, .starting:
            return Colors.warning
        case .failed:
            return Colors.danger
        case .disabled:
            return Colors.primary
        }
    }
}

@available(macOS 14.0, *)
struct SaveSection: View {
    @Binding var isSaving: Bool
    let onSave: () -> Void
    
    var body: some View {
        HStack {
            Spacer()
            
            GradientButton(isSaving ? "Saving..." : "Save Changes", icon: isSaving ? "hourglass" : "checkmark") {
                guard !isSaving else { return }
                onSave()
            }
        }
    }
}

@available(macOS 14.0, *)
struct SettingsSection<Content: View>: View {
    let title: String
    let icon: String
    @ViewBuilder let content: Content
    
    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.lg) {
            // Section header with animated divider
            HStack(spacing: Spacing.md) {
                Image(systemName: icon)
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(Colors.primary)
                
                Text(title)
                    .font(Typography.title2)
                    .foregroundColor(Colors.textPrimary)
                
                Divider()
                    .background(Colors.glassBorder)
            }
            
            // Content
            GlassCard(padding: Spacing.xl, showBorder: true, blur: .thinMaterial) {
                content
            }
        }
    }
}

@available(macOS 14.0, *)
struct CustomToggle: View {
    let title: String
    let description: String
    @Binding var isOn: Bool
    
    var body: some View {
        HStack(spacing: Spacing.lg) {
            VStack(alignment: .leading, spacing: Spacing.xs) {
                Text(title)
                    .font(Typography.subheadline)
                    .fontWeight(.medium)
                    .foregroundColor(Colors.textPrimary)
                
                Text(description)
                    .font(Typography.caption)
                    .foregroundColor(Colors.textMuted)
            }
            
            Spacer()
            
            Toggle("", isOn: $isOn)
                .toggleStyle(SwitchToggleStyle(tint: Colors.success))
                .scaleEffect(0.9)
        }
    }
}

@available(macOS 14.0, *)
struct CustomSlider: View {
    @Binding var value: Double
    let range: ClosedRange<Double>
    let step: Double
    
    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                // Track background
                RoundedRectangle(cornerRadius: 4)
                    .fill(Colors.glassBorder)
                    .frame(height: 8)
                
                // Filled track
                RoundedRectangle(cornerRadius: 4)
                    .fill(
                        LinearGradient(
                            colors: [Colors.primary, Colors.secondary],
                            startPoint: .leading,
                            endPoint: .trailing
                        )
                    )
                    .frame(width: fillWidth(in: geometry.size.width), height: 8)
                
                // Thumb
                Circle()
                    .fill(Color.white)
                    .frame(width: 20, height: 20)
                    .shadow(color: Colors.primary.opacity(0.3), radius: 8, x: 0, y: 2)
                    .offset(x: thumbPosition(in: geometry.size.width) - 10)
            }
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { gesture in
                        value = steppedValue(for: gesture.location.x, width: geometry.size.width)
                    }
            )
        }
        .frame(height: 20)
    }
    
    private func fillWidth(in totalWidth: CGFloat) -> CGFloat {
        let percentage = clampedPercentage
        return totalWidth * CGFloat(percentage)
    }
    
    private func thumbPosition(in totalWidth: CGFloat) -> CGFloat {
        let percentage = clampedPercentage
        return totalWidth * CGFloat(percentage)
    }

    private var clampedPercentage: Double {
        let raw = (value - range.lowerBound) / (range.upperBound - range.lowerBound)
        return min(max(raw, 0), 1)
    }

    private func steppedValue(for x: CGFloat, width: CGFloat) -> Double {
        guard width > 0 else { return value }
        let percentage = min(max(Double(x / width), 0), 1)
        let rawValue = range.lowerBound + percentage * (range.upperBound - range.lowerBound)
        let steps = ((rawValue - range.lowerBound) / step).rounded()
        return min(max(range.lowerBound + steps * step, range.lowerBound), range.upperBound)
    }
}

@available(macOS 14.0, *)
struct CustomTextField: View {
    let title: String
    let placeholder: String
    @Binding var text: String
    let icon: String
    var isSecure: Bool = false
    
    @FocusState private var isFocused: Bool
    
    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            Text(title)
                .font(Typography.subheadline)
                .foregroundColor(Colors.textPrimary)
            
            HStack(spacing: Spacing.md) {
                Image(systemName: icon)
                    .font(.system(size: 16))
                    .foregroundColor(isFocused ? Colors.primary : Colors.textMuted)

                Group {
                    if isSecure {
                        SecureField(placeholder, text: $text)
                    } else {
                        TextField(placeholder, text: $text)
                    }
                }
                .font(Typography.body)
                .foregroundColor(Colors.textPrimary)
                .focused($isFocused)
            }
            .padding(Spacing.md)
            .background(
                ZStack {
                    RoundedRectangle(cornerRadius: Radius.md)
                        .fill(isFocused ? Colors.surface : Colors.surfaceSecondary)
                    RoundedRectangle(cornerRadius: Radius.md)
                        .stroke(isFocused ? Colors.borderFocus : Colors.border, lineWidth: 1)
                }
            )
        }
    }
}

@available(macOS 14.0, *)
struct SavedToast: View {
    var body: some View {
        HStack(spacing: Spacing.md) {
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 20))
                .foregroundColor(Colors.success)
            
            Text("Settings saved successfully")
                .font(Typography.subheadline)
                .fontWeight(.medium)
                .foregroundColor(Colors.textPrimary)
        }
        .padding(.horizontal, Spacing.lg)
        .padding(.vertical, Spacing.md)
        .background(
            ZStack {
                RoundedRectangle(cornerRadius: Radius.lg)
                    .fill(Colors.surface)
                RoundedRectangle(cornerRadius: Radius.lg)
                    .stroke(Colors.success.opacity(0.3), lineWidth: 1)
            }
        )
        .shadow(color: Shadows.md.color, radius: Shadows.md.radius, x: Shadows.md.x, y: Shadows.md.y)
        .padding(.top, Spacing.lg)
    }
}

@available(macOS 14.0, *)
struct SettingsView_Previews: PreviewProvider {
    static var previews: some View {
        SettingsView(service: AppService())
    }
}
