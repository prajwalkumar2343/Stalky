import SwiftUI
import AppKit
import AVFoundation
import CoreGraphics

enum AppPermissionKind: String, CaseIterable, Identifiable, Hashable, Codable, Sendable {
    case screenRecording
    case accessibility
    case microphone

    var id: String { rawValue }

    var title: String {
        switch self {
        case .screenRecording:
            return "Screen Recording"
        case .accessibility:
            return "Accessibility"
        case .microphone:
            return "Microphone"
        }
    }

    var description: String {
        switch self {
        case .screenRecording:
            return "Lets Aura understand what is on screen and capture meaningful context."
        case .accessibility:
            return "Lets Aura observe app changes, shortcuts, and richer desktop state."
        case .microphone:
            return "Optional voice input for chat and future hands-free controls."
        }
    }

    var icon: String {
        switch self {
        case .screenRecording:
            return "record.circle"
        case .accessibility:
            return "figure.wave"
        case .microphone:
            return "mic"
        }
    }

    var settingsAnchor: String {
        switch self {
        case .screenRecording:
            return "Privacy_ScreenCapture"
        case .accessibility:
            return "Privacy_Accessibility"
        case .microphone:
            return "Privacy_Microphone"
        }
    }

    var actionTitle: String {
        switch self {
        case .screenRecording:
            return "Open Screen Recording"
        case .accessibility:
            return "Open Accessibility"
        case .microphone:
            return "Open Microphone"
        }
    }

    var badgeTitle: String {
        isRequired ? "Required" : "Optional"
    }

    var isRequired: Bool {
        switch self {
        case .screenRecording:
            return true
        case .accessibility, .microphone:
            return false
        }
    }

    @available(macOS 14.0, *)
    var accentColor: Color {
        switch self {
        case .screenRecording:
            return Colors.primary
        case .accessibility:
            return Colors.secondary
        case .microphone:
            return Colors.accent
        }
    }
}

struct AppPermissionStatus: Identifiable, Equatable {
    let kind: AppPermissionKind
    let state: AppPermissionState

    var id: AppPermissionKind { kind }
    var isGranted: Bool { state == .granted }
}

enum AppPermissionState: Equatable {
    case granted
    case pendingRestart
    case notGranted

    var title: String {
        switch self {
        case .granted:
            return "Enabled"
        case .pendingRestart:
            return "Restart Aura"
        case .notGranted:
            return "Open Panel"
        }
    }

    var symbolName: String {
        switch self {
        case .granted:
            return "checkmark.circle.fill"
        case .pendingRestart:
            return "arrow.clockwise.circle.fill"
        case .notGranted:
            return "arrow.up.right"
        }
    }

    var tintColor: Color {
        switch self {
        case .granted:
            return Colors.success
        case .pendingRestart:
            return Colors.warning
        case .notGranted:
            return Colors.primary
        }
    }
}

@MainActor
enum PermissionCenter {
    private static let requestedKindsDefaultsKey = "AuraBotRequestedPermissionKinds"
    private static var requestedKinds = loadRequestedKinds()
    private static var cachedAppIdentityWarning: String?
    private static var checkedAppIdentity = false

    static func allStatuses() -> [AppPermissionStatus] {
        AppPermissionKind.allCases.map(status(for:))
    }

    static func status(for kind: AppPermissionKind) -> AppPermissionStatus {
        AppPermissionStatus(kind: kind, state: state(for: kind))
    }

    static func state(for kind: AppPermissionKind) -> AppPermissionState {
        switch kind {
        case .screenRecording:
            if hasScreenRecordingAccess() {
                clearRequested(kind)
                return .granted
            }

            return requestedKinds.contains(kind) ? .pendingRestart : .notGranted
        case .accessibility:
            return isGranted(kind) ? .granted : .notGranted
        case .microphone:
            return isGranted(kind) ? .granted : .notGranted
        }
    }

    static func isGranted(_ kind: AppPermissionKind) -> Bool {
        switch kind {
        case .screenRecording:
            return hasScreenRecordingAccess()
        case .accessibility:
            return SystemAccessibilityPermissionChecker().isTrusted(prompt: false)
        case .microphone:
            return AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
        }
    }

    static func updateScreenRecordingProbe() async {
        if CGPreflightScreenCaptureAccess() {
            clearRequested(.screenRecording)
        }
    }

    private static func hasScreenRecordingAccess() -> Bool {
        CGPreflightScreenCaptureAccess()
    }

    static var appIdentityWarning: String? {
        let bundle = Bundle.main
        let bundleURL = bundle.bundleURL

        guard bundleURL.pathExtension == "app", bundle.bundleIdentifier != nil else {
            return "Aura is running from a development executable. Open the installed AuraBot.app so macOS grants permissions to one stable app entry."
        }

        guard bundleURL.path.hasPrefix("/Applications/") else {
            return "Aura is running outside Applications. Move AuraBot.app to Applications and open that copy before granting permissions to avoid duplicate macOS privacy entries."
        }

        if checkedAppIdentity {
            return cachedAppIdentityWarning
        }

        cachedAppIdentityWarning = codeSignatureWarning(for: bundleURL)
        checkedAppIdentity = true
        return cachedAppIdentityWarning
    }

    static func requestAccess(for kind: AppPermissionKind) {
        guard !isGranted(kind) else {
            clearRequested(kind)
            return
        }

        switch kind {
        case .screenRecording:
            markRequested(kind)
            let granted = CGRequestScreenCaptureAccess()
            if granted {
                if CGPreflightScreenCaptureAccess() {
                    clearRequested(kind)
                }
                return
            }
            openSystemSettings(for: kind)
        case .accessibility:
            let trusted = SystemAccessibilityPermissionChecker().isTrusted(prompt: true)
            if trusted {
                clearRequested(kind)
            } else {
                openSystemSettings(for: kind)
            }
        case .microphone:
            switch AVCaptureDevice.authorizationStatus(for: .audio) {
            case .notDetermined:
                guard hasUsageDescription("NSMicrophoneUsageDescription") else {
                    openSystemSettings(for: kind)
                    return
                }

                AVCaptureDevice.requestAccess(for: .audio) { granted in
                    guard !granted else { return }
                    Task { @MainActor in
                        openSystemSettings(for: kind)
                    }
                }
            case .denied, .restricted:
                openSystemSettings(for: kind)
            case .authorized:
                clearRequested(kind)
            @unknown default:
                openSystemSettings(for: kind)
            }
        }
    }

    @MainActor
    static func openSystemSettings(for kind: AppPermissionKind) {
        guard let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(kind.settingsAnchor)") else {
            return
        }

        NSWorkspace.shared.open(url)
    }

    private static func markRequested(_ kind: AppPermissionKind) {
        requestedKinds.insert(kind)
        persistRequestedKinds()
    }

    private static func clearRequested(_ kind: AppPermissionKind) {
        requestedKinds.remove(kind)
        persistRequestedKinds()
    }

    private static func loadRequestedKinds() -> Set<AppPermissionKind> {
        let rawValues = UserDefaults.standard.stringArray(forKey: requestedKindsDefaultsKey) ?? []
        return Set(rawValues.compactMap(AppPermissionKind.init(rawValue:)))
    }

    private static func persistRequestedKinds() {
        UserDefaults.standard.set(
            requestedKinds.map(\.rawValue).sorted(),
            forKey: requestedKindsDefaultsKey
        )
    }

    private static func hasUsageDescription(_ key: String) -> Bool {
        guard let value = Bundle.main.object(forInfoDictionaryKey: key) as? String else {
            return false
        }

        return !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private static func codeSignatureWarning(for bundleURL: URL) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/codesign")
        process.arguments = ["-d", "--verbose=4", bundleURL.path]

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe

        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            return "Aura could not verify its code signature. Use a Developer ID signed and notarized build so macOS can preserve privacy permissions reliably."
        }

        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        let output = String(data: data, encoding: .utf8) ?? ""
        if output.contains("Signature=adhoc") || output.contains("TeamIdentifier=not set") {
            return "Aura is running from a local ad-hoc build. macOS may not preserve Screen Recording reliably across rebuilds; use a Developer ID signed and notarized build for stable permissions."
        }

        return nil
    }
}

@available(macOS 14.0, *)
struct PermissionOnboardingView: View {
    @ObservedObject var service: AppService
    @State private var selectedStep: OnboardingStep = .welcome
    @State private var isCompleting = false
    @State private var completionError: String?

    private var requiredStatuses: [AppPermissionStatus] {
        service.permissionStatuses.filter { $0.kind.isRequired }
    }

    private var completedRequiredCount: Int {
        requiredStatuses.filter { $0.isGranted }.count
    }

    private var progressValue: Double {
        guard !requiredStatuses.isEmpty else { return 1 }
        return Double(completedRequiredCount) / Double(requiredStatuses.count)
    }

    private var allRequiredGranted: Bool {
        completedRequiredCount == requiredStatuses.count
    }

    var body: some View {
        VStack(spacing: 0) {
            OnboardingStepHeader(
                selectedStep: selectedStep,
                completedRequiredCount: completedRequiredCount,
                totalRequiredCount: requiredStatuses.count,
                onSelectStep: move(to:)
            )
                .padding(.horizontal, Spacing.xxxxl)
                .padding(.top, Spacing.xxxxl)

            ZStack {
                switch selectedStep {
                case .welcome:
                    OnboardingWelcomeScreen(
                        progressValue: progressValue,
                        completedRequiredCount: completedRequiredCount,
                        totalRequiredCount: requiredStatuses.count,
                        onContinue: { move(to: .permissions) }
                    )
                    .transition(.opacity.combined(with: .move(edge: .leading)))
                case .permissions:
                    OnboardingPermissionsScreen(
                        statuses: service.permissionStatuses,
                        completedRequiredCount: completedRequiredCount,
                        totalRequiredCount: requiredStatuses.count,
                        progressValue: progressValue,
                        guidanceMessage: service.permissionGuidanceMessage,
                        onRequest: { kind in
                            service.requestPermission(kind)
                        },
                        onContinue: {
                            move(to: .browserExtension)
                        }
                    )
                    .transition(.opacity)
                case .browserExtension:
                    OnboardingBrowserScreen(
                        service: service,
                        canFinish: allRequiredGranted,
                        isCompleting: isCompleting,
                        completionError: completionError,
                        onBack: { move(to: .permissions) },
                        onFinish: finishOnboarding
                    )
                    .transition(.opacity.combined(with: .move(edge: .trailing)))
                }
            }
            .padding(.horizontal, Spacing.xxxxl)
            .padding(.bottom, Spacing.xxxxl)
        }
        .background(onboardingBackground)
        .onAppear {
            service.refreshPermissionStatuses()
            selectedStep = service.requiredPermissionsGranted ? .browserExtension : .welcome
        }
        .onChange(of: allRequiredGranted) { _, granted in
            guard granted, selectedStep == .permissions else { return }
            move(to: .browserExtension)
        }
        .onChange(of: selectedStep) { _, _ in
            completionError = nil
        }
    }

    private var onboardingBackground: some View {
        ZStack {
            Colors.background

            LinearGradient(
                colors: [
                    Colors.primary.opacity(0.08),
                    Colors.secondary.opacity(0.05),
                    Color.clear
                ],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
        }
    }

    private func move(to step: OnboardingStep) {
        withAnimation(AnimationPresets.spring) {
            selectedStep = step
        }
    }

    private func finishOnboarding() {
        guard allRequiredGranted, !isCompleting else {
            if !allRequiredGranted {
                move(to: .permissions)
            }
            return
        }

        isCompleting = true
        completionError = nil

        Task {
            do {
                try await service.completeOnboarding()
                await MainActor.run {
                    isCompleting = false
                }
            } catch {
                await MainActor.run {
                    completionError = "Could not save onboarding state. Try again."
                    isCompleting = false
                }
            }
        }
    }
}

@available(macOS 14.0, *)
private enum OnboardingStep: Int, CaseIterable, Identifiable {
    case welcome
    case permissions
    case browserExtension

    var id: Int { rawValue }

    var number: Int { rawValue + 1 }

    var title: String {
        switch self {
        case .welcome:
            return "Welcome"
        case .permissions:
            return "Permissions"
        case .browserExtension:
            return "Browser"
        }
    }
}

@available(macOS 14.0, *)
private struct OnboardingStepHeader: View {
    let selectedStep: OnboardingStep
    let completedRequiredCount: Int
    let totalRequiredCount: Int
    let onSelectStep: (OnboardingStep) -> Void

    var body: some View {
        HStack(spacing: Spacing.lg) {
            ForEach(OnboardingStep.allCases) { step in
                Button {
                    onSelectStep(step)
                } label: {
                    HStack(spacing: Spacing.sm) {
                        Text("\(step.number)")
                            .font(Typography.caption.weight(.bold))
                            .foregroundColor(step.rawValue <= selectedStep.rawValue ? Colors.textInverse : Colors.textSecondary)
                            .frame(width: 24, height: 24)
                            .background(
                                Circle()
                                    .fill(step.rawValue <= selectedStep.rawValue ? Colors.primary : Colors.surfaceTertiary)
                            )

                        Text(step.title)
                            .font(Typography.subheadline.weight(step == selectedStep ? .semibold : .regular))
                            .foregroundColor(step == selectedStep ? Colors.textPrimary : Colors.textSecondary)
                    }
                }
                .buttonStyle(.plain)

                if step != OnboardingStep.allCases.last {
                    Rectangle()
                        .fill(step.rawValue < selectedStep.rawValue ? Colors.primary.opacity(0.55) : Colors.border)
                        .frame(height: 1)
                }
            }

            Spacer()

            Text("\(completedRequiredCount)/\(totalRequiredCount) required")
                .font(Typography.caption.weight(.semibold))
                .foregroundColor(completedRequiredCount == totalRequiredCount ? Colors.success : Colors.primary)
                .padding(.horizontal, Spacing.sm)
                .padding(.vertical, Spacing.xs)
                .background(
                    Capsule()
                        .fill(completedRequiredCount == totalRequiredCount ? Colors.success.opacity(0.12) : Colors.primaryMuted)
                )
        }
    }
}

@available(macOS 14.0, *)
private struct OnboardingWelcomeScreen: View {
    let progressValue: Double
    let completedRequiredCount: Int
    let totalRequiredCount: Int
    let onContinue: () -> Void

    var body: some View {
        OnboardingScreenContainer {
            HStack(alignment: .center, spacing: Spacing.xxxxl) {
                VStack(alignment: .leading, spacing: Spacing.xl) {
                    onboardingBadge("Setup", icon: "sparkles")

                    VStack(alignment: .leading, spacing: Spacing.md) {
                        Text("Set up Aura in three steps.")
                            .font(Typography.largeTitle)
                            .foregroundColor(Colors.textPrimary)
                            .fixedSize(horizontal: false, vertical: true)

                        Text("Aura needs Screen Recording before it can visually understand your workspace. The next screen checks status as you return from System Settings and shows when a restart is needed.")
                            .font(Typography.body)
                            .foregroundColor(Colors.textSecondary)
                            .frame(maxWidth: 560, alignment: .leading)
                    }

                    GradientButton("Continue", icon: "arrow.right") {
                        onContinue()
                    }
                }

                PermissionProgressCard(
                    completedCount: completedRequiredCount,
                    totalCount: totalRequiredCount,
                    progressValue: progressValue
                )
                .frame(width: 260)
            }
        }
    }
}

@available(macOS 14.0, *)
private struct OnboardingPermissionsScreen: View {
    let statuses: [AppPermissionStatus]
    let completedRequiredCount: Int
    let totalRequiredCount: Int
    let progressValue: Double
    let guidanceMessage: String?
    let onRequest: (AppPermissionKind) -> Void
    let onContinue: () -> Void

    private var allRequiredGranted: Bool {
        completedRequiredCount == totalRequiredCount
    }

    private var requiredStatuses: [AppPermissionStatus] {
        statuses.filter { $0.kind.isRequired }
    }

    private var optionalStatuses: [AppPermissionStatus] {
        statuses.filter { !$0.kind.isRequired }
    }

    var body: some View {
        OnboardingScreenContainer {
            HStack(alignment: .top, spacing: Spacing.xxxl) {
                VStack(alignment: .leading, spacing: Spacing.xl) {
                    onboardingBadge("Live Status", icon: "arrow.triangle.2.circlepath")

                    VStack(alignment: .leading, spacing: Spacing.sm) {
                        Text("Grant Screen Recording.")
                            .font(Typography.title1)
                            .foregroundColor(Colors.textPrimary)

                        Text(guidanceMessage ?? "Aura unlocks once Screen Recording is enabled.")
                            .font(Typography.callout)
                            .foregroundColor(Colors.textSecondary)
                    }

                    GlassCard(padding: Spacing.xl, cornerRadius: Radius.xl, shadow: Shadows.md, showBorder: true) {
                        VStack(alignment: .leading, spacing: Spacing.md) {
                            ForEach(requiredStatuses) { status in
                                PermissionChecklistRow(status: status) {
                                    onRequest(status.kind)
                                }
                            }

                            if !optionalStatuses.isEmpty {
                                Text("Optional")
                                    .font(Typography.caption.weight(.semibold))
                                    .foregroundColor(Colors.textSecondary)
                                    .padding(.top, Spacing.xs)

                                ForEach(optionalStatuses) { status in
                                    PermissionChecklistRow(status: status) {
                                        onRequest(status.kind)
                                    }
                                }
                            }
                        }
                    }
                }

                VStack(alignment: .leading, spacing: Spacing.lg) {
                    PermissionProgressCard(
                        completedCount: completedRequiredCount,
                        totalCount: totalRequiredCount,
                        progressValue: progressValue
                    )

                    Group {
                        GradientButton(allRequiredGranted ? "Continue" : "Waiting", icon: allRequiredGranted ? "arrow.right" : "hourglass") {
                            guard allRequiredGranted else {
                                return
                            }
                            onContinue()
                        }
                        .disabled(!allRequiredGranted)
                    }
                }
                .frame(width: 260)
            }
        }
    }
}

@available(macOS 14.0, *)
private struct OnboardingBrowserScreen: View {
    @ObservedObject var service: AppService
    let canFinish: Bool
    let isCompleting: Bool
    let completionError: String?
    let onBack: () -> Void
    let onFinish: () -> Void

    var body: some View {
        OnboardingScreenContainer {
            VStack(alignment: .leading, spacing: Spacing.xl) {
                HStack(alignment: .top) {
                    VStack(alignment: .leading, spacing: Spacing.md) {
                        onboardingBadge("Final Step", icon: "puzzlepiece.extension")

                        Text("Connect browser context.")
                            .font(Typography.title1)
                            .foregroundColor(Colors.textPrimary)

                        Text("This step is optional, but Chrome context gives Aura better page awareness. You can finish now and revisit extension setup from Settings.")
                            .font(Typography.callout)
                            .foregroundColor(Colors.textSecondary)
                            .frame(maxWidth: 620, alignment: .leading)
                    }

                    Spacer()

                    Text(canFinish ? "Ready" : "Permissions needed")
                        .font(Typography.caption.weight(.semibold))
                        .foregroundColor(canFinish ? Colors.success : Colors.warning)
                        .padding(.horizontal, Spacing.sm)
                        .padding(.vertical, Spacing.xs)
                        .background(
                            Capsule()
                                .fill(canFinish ? Colors.success.opacity(0.12) : Colors.warning.opacity(0.12))
                        )
                }

                BrowserExtensionSetupCard(service: service)

                if let completionError {
                    Text(completionError)
                        .font(Typography.caption)
                        .foregroundColor(Colors.danger)
                }

                HStack(spacing: Spacing.md) {
                    SecondaryButton("Back", icon: "chevron.left") {
                        onBack()
                    }

                    Spacer()

                    GradientButton(isCompleting ? "Finishing..." : "Finish Setup", icon: isCompleting ? "hourglass" : "checkmark") {
                        onFinish()
                    }
                    .opacity(canFinish ? 1 : 0.55)
                }
            }
        }
    }
}

@available(macOS 14.0, *)
private struct OnboardingScreenContainer<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        ScrollView(showsIndicators: false) {
            content
                .padding(.top, Spacing.xxxl)
                .padding(.bottom, Spacing.xxxl)
                .frame(maxWidth: .infinity, alignment: .topLeading)
        }
    }
}

@available(macOS 14.0, *)
private func onboardingBadge(_ title: String, icon: String) -> some View {
    HStack(spacing: Spacing.sm) {
        Image(systemName: icon)
            .font(.system(size: 12, weight: .semibold))
            .foregroundColor(Colors.primary)

        Text(title)
            .font(Typography.caption.weight(.semibold))
            .foregroundColor(Colors.textSecondary)
    }
    .padding(.horizontal, Spacing.md)
    .padding(.vertical, Spacing.sm)
    .background(
        Capsule()
            .fill(Colors.surface)
            .overlay(
                Capsule()
                    .stroke(Colors.border, lineWidth: 1)
            )
    )
}

@available(macOS 14.0, *)
struct BrowserExtensionSetupCard: View {
    @ObservedObject var service: AppService

    var body: some View {
        GlassCard(padding: Spacing.xxl, cornerRadius: 28, shadow: Shadows.lg, showBorder: true) {
            VStack(alignment: .leading, spacing: Spacing.lg) {
                HStack(alignment: .top) {
                    VStack(alignment: .leading, spacing: Spacing.xs) {
                        Text("Chrome Extension")
                            .font(Typography.title2)
                            .foregroundColor(Colors.textPrimary)

                        Text("Optional, but recommended for richer browser context and page awareness inside Chrome.")
                            .font(Typography.callout)
                            .foregroundColor(Colors.textSecondary)
                    }

                    Spacer()

                    Text(service.browserExtensionConfigured ? "Configured" : "Recommended")
                        .font(Typography.caption)
                        .foregroundColor(service.browserExtensionConfigured ? Colors.success : Colors.accent)
                        .padding(.horizontal, Spacing.sm)
                        .padding(.vertical, Spacing.xs)
                        .background(
                            Capsule()
                                .fill(service.browserExtensionConfigured ? Colors.success.opacity(0.12) : Colors.accent.opacity(0.12))
                        )
                }

                Button {
                    service.installChromeExtension()
                } label: {
                    HStack(spacing: Spacing.lg) {
                        ZStack {
                            RoundedRectangle(cornerRadius: 16)
                                .fill(Colors.accent.opacity(0.12))
                                .frame(width: 52, height: 52)

                            Image(systemName: "puzzlepiece.extension")
                                .font(.system(size: 22, weight: .semibold))
                                .foregroundColor(Colors.accent)
                        }

                        VStack(alignment: .leading, spacing: Spacing.xs) {
                            HStack(spacing: Spacing.sm) {
                                Text("Install Aura In Chrome")
                                    .font(Typography.headline)
                                    .foregroundColor(Colors.textPrimary)

                                Text("Optional")
                                    .font(Typography.caption2)
                                    .foregroundColor(Colors.textSecondary)
                                    .padding(.horizontal, Spacing.sm)
                                    .padding(.vertical, 3)
                                    .background(
                                        Capsule()
                                            .fill(Colors.surfaceTertiary)
                                    )
                            }

                            Text("Launches Chrome with the Aura extension loaded so the browser context bridge is ready immediately.")
                                .font(Typography.callout)
                                .foregroundColor(Colors.textSecondary)
                        }

                        Spacer()

                        VStack(alignment: .trailing, spacing: Spacing.xs) {
                            Text(service.hasChromeExtensionBundle ? "Open Chrome" : "Open Setup")
                                .font(Typography.caption)
                                .foregroundColor(Colors.accent)

                            Image(systemName: "arrow.up.right")
                                .font(.system(size: 13, weight: .bold))
                                .foregroundColor(Colors.accent)
                        }
                    }
                    .padding(Spacing.lg)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(
                        RoundedRectangle(cornerRadius: 22)
                            .fill(Colors.surface)
                            .overlay(
                                RoundedRectangle(cornerRadius: 22)
                                    .stroke(Colors.border, lineWidth: 1)
                            )
                    )
                }
                .buttonStyle(.plain)

                VStack(alignment: .leading, spacing: Spacing.md) {
                    BrowserExtensionStep(
                        index: "1",
                        title: "Install flow",
                        detail: service.hasChromeExtensionBundle ? "Aura opens Chrome and loads the bundled extension automatically." : "Aura opens Chrome setup so you can finish loading the extension manually."
                    )
                    BrowserExtensionStep(
                        index: "2",
                        title: "API key",
                        detail: service.browserExtensionConfigured ? "Use the Browser Extension API key from Settings." : "Add an API key in Settings first, then paste it into the extension options."
                    )
                    BrowserExtensionStep(
                        index: "3",
                        title: "Local server URL",
                        detail: service.browserExtensionServerURL,
                        isMono: true
                    )
                }
            }
        }
    }
}

@available(macOS 14.0, *)
struct BrowserExtensionStep: View {
    let index: String
    let title: String
    let detail: String
    var isMono: Bool = false

    var body: some View {
        HStack(alignment: .top, spacing: Spacing.md) {
            Text(index)
                .font(Typography.caption.weight(.semibold))
                .foregroundColor(Colors.textInverse)
                .frame(width: 22, height: 22)
                .background(Circle().fill(Colors.accent))

            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(Typography.subheadline.weight(.semibold))
                    .foregroundColor(Colors.textPrimary)

                Text(detail)
                    .font(isMono ? Typography.mono : Typography.callout)
                    .foregroundColor(Colors.textSecondary)
                    .textSelection(.enabled)
            }
        }
    }
}

@available(macOS 14.0, *)
struct PermissionProgressCard: View {
    let completedCount: Int
    let totalCount: Int
    let progressValue: Double

    private var visibleProgressValue: Double {
        max(0.02, progressValue)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.lg) {
            ZStack {
                Circle()
                    .stroke(Colors.border, lineWidth: 14)

                Circle()
                    .trim(from: 0, to: visibleProgressValue)
                    .stroke(
                        AngularGradient(
                            colors: [Colors.primary, Colors.secondary, Colors.primary],
                            center: .center
                        ),
                        style: StrokeStyle(lineWidth: 14, lineCap: .round)
                    )
                    .rotationEffect(.degrees(-90))

                VStack(spacing: Spacing.xs) {
                    Text("\(completedCount)")
                        .font(.system(size: 36, weight: .bold))
                        .foregroundColor(Colors.textPrimary)

                    Text("of \(totalCount)")
                        .font(Typography.caption)
                        .foregroundColor(Colors.textSecondary)
                }
            }
            .frame(width: 132, height: 132)

            Text("Required Access")
                .font(Typography.headline)
                .foregroundColor(Colors.textPrimary)

            Text(completedCount == totalCount ? "Aura is ready to start." : "Grant the remaining items to continue.")
                .font(Typography.callout)
                .foregroundColor(Colors.textSecondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(Spacing.xxl)
        .background(
            RoundedRectangle(cornerRadius: 28)
                .fill(Colors.surface.opacity(0.95))
                .overlay(
                    RoundedRectangle(cornerRadius: 28)
                        .stroke(Colors.border, lineWidth: 1)
                )
        )
        .shadow(color: Shadows.lg.color, radius: Shadows.lg.radius, x: Shadows.lg.x, y: Shadows.lg.y)
    }
}

@available(macOS 14.0, *)
struct PermissionChecklistGroup: View {
    let statuses: [AppPermissionStatus]
    let onTap: (AppPermissionKind) -> Void

    var body: some View {
        VStack(spacing: Spacing.md) {
            ForEach(statuses) { status in
                PermissionChecklistRow(status: status) {
                    onTap(status.kind)
                }
            }
        }
    }
}

@available(macOS 14.0, *)
struct PermissionChecklistRow: View {
    let status: AppPermissionStatus
    let onTap: () -> Void

    @State private var isHovered = false

    var body: some View {
        Button {
            guard !status.isGranted else { return }
            onTap()
        } label: {
            HStack(spacing: Spacing.lg) {
                ZStack {
                    RoundedRectangle(cornerRadius: 16)
                        .fill(statusIconColor.opacity(status.state == .granted ? 0.18 : 0.10))
                        .frame(width: 52, height: 52)

                    Image(systemName: status.state == .notGranted ? status.kind.icon : status.state.symbolName)
                        .font(.system(size: 22, weight: .semibold))
                        .foregroundColor(statusIconColor)
                }

                VStack(alignment: .leading, spacing: Spacing.xs) {
                    HStack(spacing: Spacing.sm) {
                        Text(status.kind.title)
                            .font(Typography.headline)
                            .foregroundColor(Colors.textPrimary)

                        Text(status.kind.badgeTitle)
                            .font(Typography.caption2)
                            .foregroundColor(status.kind.isRequired ? Colors.primary : Colors.textSecondary)
                            .padding(.horizontal, Spacing.sm)
                            .padding(.vertical, 3)
                            .background(
                                Capsule()
                                    .fill(status.kind.isRequired ? Colors.primaryMuted : Colors.surfaceTertiary)
                            )
                    }

                    Text(status.kind.description)
                        .font(Typography.callout)
                        .foregroundColor(Colors.textSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                }

                Spacer()

                VStack(alignment: .trailing, spacing: Spacing.xs) {
                    Text(status.state.title)
                        .font(Typography.caption)
                        .foregroundColor(status.state.tintColor)

                    Image(systemName: trailingSymbolName)
                        .font(.system(size: 13, weight: .bold))
                        .foregroundColor(status.state.tintColor)
                }
            }
            .padding(Spacing.lg)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: 22)
                    .fill(isHovered ? Colors.surfaceSecondary : Colors.surface)
                    .overlay(
                        RoundedRectangle(cornerRadius: 22)
                            .stroke(borderColor, lineWidth: 1)
                    )
            )
        }
        .buttonStyle(.plain)
        .scaleEffect(isHovered && !status.isGranted ? 1.01 : 1.0)
        .animation(AnimationPresets.hover, value: isHovered)
        .onHover { isHovered = $0 }
    }

    private var statusIconColor: Color {
        switch status.state {
        case .granted:
            return Colors.success
        case .pendingRestart:
            return Colors.warning
        case .notGranted:
            return status.kind.accentColor
        }
    }

    private var trailingSymbolName: String {
        switch status.state {
        case .granted:
            return "checkmark"
        case .pendingRestart:
            return "arrow.clockwise"
        case .notGranted:
            return "arrow.up.right"
        }
    }

    private var borderColor: Color {
        switch status.state {
        case .granted:
            return Colors.success.opacity(0.28)
        case .pendingRestart:
            return Colors.warning.opacity(0.28)
        case .notGranted:
            return isHovered ? Colors.borderHover : Colors.border
        }
    }
}
