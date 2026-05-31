import SwiftUI
import AppKit

@available(macOS 14.0, *)
struct ContentView: View {
    @ObservedObject var service: AppService
    @State private var selectedTab: SidebarTab = .dashboard
    @State private var isLoading = true
    
    var body: some View {
        ZStack {
            MainContentView(service: service, selectedTab: $selectedTab)
                .opacity(isLoading ? 0 : 1)
            
            if isLoading {
                LoadingScreen()
                    .transition(.opacity)
                    .zIndex(1)
            }
        }
        .onAppear {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
                withAnimation(.easeOut(duration: 0.25)) {
                    isLoading = false
                }
            }
        }
    }
}

@available(macOS 14.0, *)
struct LoadingScreen: View {
    @State private var opacity = 0.0
    
    var body: some View {
        ZStack {
            Colors.background
            
            VStack(spacing: Spacing.lg) {
                Image(systemName: "brain.head.profile")
                    .font(.system(size: 48, weight: .light))
                    .foregroundColor(Colors.primary)
                
                Text("Aura")
                    .font(Typography.title1)
                    .foregroundColor(Colors.textPrimary)
            }
            .opacity(opacity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            withAnimation(.easeOut(duration: 0.25)) {
                opacity = 1.0
            }
        }
    }
}

@available(macOS 14.0, *)
struct MainContentView: View {
    @ObservedObject var service: AppService
    @Binding var selectedTab: SidebarTab
    
    var body: some View {
        HStack(spacing: 0) {
            SidebarView(selectedTab: $selectedTab, service: service)
            
            ZStack {
                if service.needsOnboarding, selectedTab != .settings {
                    PermissionOnboardingView(service: service)
                        .transition(.opacity.combined(with: .scale(scale: 0.98)))
                } else if case let .pluginWorkspace(pluginID, name) = service.appPresentation.mode {
                    PluginWorkspaceView(
                        pluginID: pluginID,
                        pluginName: name,
                        plugin: service.activePlugin,
                        service: service
                    )
                    .transition(.opacity.combined(with: .scale(scale: 0.98)))
                } else {
                    switch selectedTab {
                    case .dashboard:
                        DashboardView(service: service)
                    case .memories:
                        MemoriesView(service: service)
                    case .settings:
                        SettingsView(service: service)
                    }
                }
            }
            .background(Colors.background)
            .cornerRadius(Radius.xl)
            .padding(.vertical, Spacing.lg)
            .padding(.trailing, Spacing.lg)
        }
        .background(Colors.background)
        .onAppear {
            service.refreshPermissionStatuses()
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            service.refreshPermissionStatuses()
        }
        .onChange(of: service.needsOnboarding) { _, needsOnboarding in
            if !needsOnboarding {
                selectedTab = .dashboard
            }
        }
    }
}

@available(macOS 14.0, *)
struct PluginWorkspaceView: View {
    let pluginID: String
    let pluginName: String
    let plugin: InstalledPluginRecord?
    @ObservedObject var service: AppService

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.lg) {
            HStack {
                VStack(alignment: .leading, spacing: Spacing.xs) {
                    Text(pluginName)
                        .font(Typography.title1)
                        .foregroundColor(Colors.textPrimary)

                    Text(pluginID)
                        .font(Typography.caption)
                        .foregroundColor(Colors.textSecondary)
                }

                Spacer()

                Button("Return to Aura") {
                    Task {
                        await service.deactivateWorkspacePlugin(pluginID: pluginID)
                    }
                }
                .buttonStyle(.borderedProminent)
            }

            GlassCard {
                if let plugin, plugin.manifest.onboarding.required, !plugin.onboardingCompleted {
                    PluginOnboardingSurface(plugin: plugin) {
                        Task {
                            try? await service.completeActivePluginOnboarding()
                        }
                    }
                } else if let plugin {
                    PluginWorkspacePanel(
                        title: plugin.manifest.presentation.workspaceTitle,
                        icon: plugin.manifest.presentation.workspaceIcon,
                        rows: plugin.manifest.presentation.workspaceSections
                    )
                } else {
                    PluginWorkspacePanel(
                        title: pluginName,
                        icon: "puzzlepiece.extension",
                        rows: ["Workspace", "Context", "Actions"]
                    )
                }
            }

            Spacer()
        }
        .padding(Spacing.xl)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

@available(macOS 14.0, *)
struct PluginOnboardingSurface: View {
    let plugin: InstalledPluginRecord
    let onComplete: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.xl) {
            HStack(spacing: Spacing.md) {
                Image(systemName: plugin.manifest.presentation.workspaceIcon)
                    .font(.system(size: 22, weight: .semibold))
                    .foregroundColor(Colors.primary)
                    .frame(width: 44, height: 44)
                    .background(Colors.primaryMuted)
                    .cornerRadius(Radius.md)

                VStack(alignment: .leading, spacing: Spacing.xs) {
                    Text(plugin.manifest.onboarding.title)
                        .font(Typography.title2)
                        .foregroundColor(Colors.textPrimary)

                    Text(plugin.manifest.onboarding.detail)
                        .font(Typography.callout)
                        .foregroundColor(Colors.textSecondary)
                }
            }

            if !plugin.manifest.onboarding.requiredHostPermissions.isEmpty {
                VStack(alignment: .leading, spacing: Spacing.sm) {
                    Text("Plugin-requested permissions")
                        .font(Typography.headline)
                        .foregroundColor(Colors.textPrimary)

                    ForEach(plugin.manifest.onboarding.requiredHostPermissions) { permission in
                        HStack(spacing: Spacing.sm) {
                            Image(systemName: permission.icon)
                                .foregroundColor(permission.accentColor)

                            Text(permission.title)
                                .font(Typography.callout)
                                .foregroundColor(Colors.textSecondary)
                        }
                    }
                }
            }

            VStack(alignment: .leading, spacing: Spacing.sm) {
                ForEach(Array(plugin.manifest.onboarding.steps.enumerated()), id: \.offset) { index, step in
                    HStack(alignment: .top, spacing: Spacing.md) {
                        Text("\(index + 1)")
                            .font(Typography.caption.weight(.bold))
                            .foregroundColor(Colors.textInverse)
                            .frame(width: 22, height: 22)
                            .background(Circle().fill(Colors.primary))

                        Text(step)
                            .font(Typography.callout)
                            .foregroundColor(Colors.textSecondary)
                    }
                }
            }

            GradientButton("Complete Plugin Setup", icon: "checkmark") {
                onComplete()
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

@available(macOS 14.0, *)
struct PluginWorkspacePanel: View {
    let title: String
    let icon: String
    let rows: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.xl) {
            HStack(spacing: Spacing.md) {
                Image(systemName: icon)
                    .font(.system(size: 22, weight: .semibold))
                    .foregroundColor(Colors.primary)
                    .frame(width: 44, height: 44)
                    .background(Colors.primaryMuted)
                    .cornerRadius(Radius.md)

                Text(title)
                    .font(Typography.title2)
                    .foregroundColor(Colors.textPrimary)
            }

            LazyVGrid(
                columns: [GridItem(.adaptive(minimum: 220), spacing: Spacing.lg)],
                alignment: .leading,
                spacing: Spacing.lg
            ) {
                ForEach(rows, id: \.self) { row in
                    VStack(alignment: .leading, spacing: Spacing.sm) {
                        Text(row)
                            .font(Typography.headline)
                            .foregroundColor(Colors.textPrimary)

                        Text("Plugin-owned surface")
                            .font(Typography.caption)
                            .foregroundColor(Colors.textSecondary)
                    }
                    .frame(maxWidth: .infinity, minHeight: 112, alignment: .topLeading)
                    .padding(Spacing.lg)
                    .background(Colors.surfaceSecondary)
                    .overlay(
                        RoundedRectangle(cornerRadius: Radius.lg)
                            .stroke(Colors.border, lineWidth: 1)
                    )
                    .cornerRadius(Radius.lg)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

@available(macOS 14.0, *)
struct ContentView_Previews: PreviewProvider {
    static var previews: some View {
        ContentView(service: AppService())
            .frame(width: 1200, height: 800)
    }
}
