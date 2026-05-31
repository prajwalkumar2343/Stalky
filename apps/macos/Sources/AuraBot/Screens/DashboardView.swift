import SwiftUI

@available(macOS 14.0, *)
struct DashboardView: View {
    @ObservedObject var service: AppService
    @State private var installingPluginID: String?
    @State private var installError: String?
    
    var body: some View {
        ScrollView(showsIndicators: false) {
            VStack(alignment: .leading, spacing: Spacing.xxxl) {
                VStack(alignment: .leading, spacing: Spacing.sm) {
                    Text("Plugins")
                        .font(Typography.title1)
                        .foregroundColor(Colors.textPrimary)

                    Text("Install one plugin to shape the Aura interface.")
                        .font(Typography.callout)
                        .foregroundColor(Colors.textSecondary)
                }

                HStack(spacing: Spacing.md) {
                    Button {
                        Task {
                            await service.refreshPluginCatalog()
                        }
                    } label: {
                        Label(catalogRefreshTitle, systemImage: "arrow.clockwise")
                    }
                    .buttonStyle(SecondaryButtonStyle())

                    if case let .failed(message) = service.pluginCatalogStatus {
                        Text(message)
                            .font(Typography.caption)
                            .foregroundColor(Colors.warning)
                    }
                }

                LazyVGrid(
                    columns: [GridItem(.adaptive(minimum: 260), spacing: Spacing.lg)],
                    alignment: .leading,
                    spacing: Spacing.lg
                ) {
                    ForEach(service.availablePlugins) { plugin in
                        PluginInstallCard(
                            plugin: plugin,
                            installedVersion: service.installedPlugins.first(where: { $0.pluginID == plugin.id })?.version,
                            isInstalling: installingPluginID == plugin.id
                        ) {
                            install(plugin)
                        }
                    }
                }

                if let installError {
                    Text(installError)
                        .font(Typography.caption)
                        .foregroundColor(Colors.danger)
                }
            }
            .padding(Spacing.xxxl)
        }
        .background(Color.clear)
    }

    private var catalogRefreshTitle: String {
        switch service.pluginCatalogStatus {
        case .loading:
            return "Refreshing"
        default:
            return "Refresh Catalog"
        }
    }

    private func install(_ plugin: WorkspacePluginCatalogItem) {
        guard installingPluginID == nil else { return }
        installingPluginID = plugin.id
        installError = nil

        Task {
            do {
                try await service.installWorkspacePlugin(pluginID: plugin.id)
                await MainActor.run {
                    installingPluginID = nil
                }
            } catch {
                await MainActor.run {
                    installingPluginID = nil
                    installError = "Could not install \(plugin.name)."
                }
            }
        }
    }
}

@available(macOS 14.0, *)
struct PluginInstallCard: View {
    let plugin: WorkspacePluginCatalogItem
    let installedVersion: String?
    let isInstalling: Bool
    let onInstall: () -> Void

    private var isInstalled: Bool {
        installedVersion != nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.xl) {
            HStack {
                Image(systemName: plugin.icon)
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundColor(Colors.primary)
                    .frame(width: 40, height: 40)
                    .background(Colors.primaryMuted)
                    .cornerRadius(Radius.md)

                Spacer()

                Text(isInstalled ? "Installed" : "Available")
                    .font(Typography.caption.weight(.semibold))
                    .foregroundColor(isInstalled ? Colors.success : Colors.textSecondary)
            }

            VStack(alignment: .leading, spacing: Spacing.sm) {
                Text(plugin.name)
                    .font(Typography.title3)
                    .foregroundColor(Colors.textPrimary)

                Text(plugin.summary)
                    .font(Typography.callout)
                    .foregroundColor(Colors.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)

                Text(isInstalled ? "Installed \(installedVersion ?? plugin.version)" : "Version \(plugin.version)")
                    .font(Typography.caption)
                    .foregroundColor(Colors.textTertiary)
            }

            Spacer()

            Button {
                onInstall()
            } label: {
                Label(isInstalling ? "Installing" : (isInstalled ? "Open" : "Install"), systemImage: isInstalling ? "hourglass" : (isInstalled ? "arrow.right.circle" : "arrow.down.circle"))
            }
            .buttonStyle(PrimaryButtonStyle())
            .disabled(isInstalling)
        }
        .frame(maxWidth: .infinity, minHeight: 220, alignment: .leading)
        .padding(Spacing.xl)
        .background(Colors.surface)
        .overlay(
            RoundedRectangle(cornerRadius: Radius.xl)
                .stroke(isInstalled ? Colors.success.opacity(0.32) : Colors.border, lineWidth: 1)
        )
        .cornerRadius(Radius.xl)
        .shadow(color: Shadows.sm.color, radius: Shadows.sm.radius, x: Shadows.sm.x, y: Shadows.sm.y)
    }
}

@available(macOS 14.0, *)
struct InlinePermissionNotice: View {
    let message: String

    var body: some View {
        HStack(spacing: Spacing.sm) {
            Image(systemName: "lock.trianglebadge.exclamationmark")
                .foregroundColor(Colors.warning)

            Text(message)
                .font(Typography.caption)
                .foregroundColor(Colors.warning)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, Spacing.md)
        .padding(.vertical, Spacing.sm)
        .background(
            RoundedRectangle(cornerRadius: Radius.md)
                .fill(Colors.warning.opacity(0.10))
                .overlay(
                    RoundedRectangle(cornerRadius: Radius.md)
                        .stroke(Colors.warning.opacity(0.25), lineWidth: 1)
                )
        )
    }
}

@available(macOS 14.0, *)
struct StatsGrid: View {
    @ObservedObject var service: AppService

    private var capturesToday: Int {
        service.memories.filter { Calendar.current.isDateInToday($0.createdAt) }.count
    }
    
    var body: some View {
        LazyVGrid(
            columns: [GridItem(.adaptive(minimum: 210), spacing: Spacing.lg)],
            alignment: .leading,
            spacing: Spacing.lg
        ) {
            DashboardStatCard(
                title: "Memories",
                value: "\(service.memories.count)",
                detail: "\(capturesToday) today",
                icon: "brain.head.profile",
                color: Colors.primary
            )

            DashboardStatCard(
                title: "Capture",
                value: "\(service.captureInterval)s",
                detail: service.captureEnabled ? "Active interval" : "Paused",
                icon: "timer",
                color: Colors.secondary
            )

            DashboardStatCard(
                title: "LLM",
                value: service.isLLMConnected ? "Online" : "Offline",
                detail: service.llmStatusMessage ?? (service.config.llm.model.isEmpty ? "No model set" : service.config.llm.model),
                icon: "sparkles",
                color: service.isLLMConnected ? Colors.success : Colors.warning
            )

            DashboardStatCard(
                title: "Memory API",
                value: service.isMemoryConnected ? "Online" : "Offline",
                detail: service.memoryStatusMessage ?? service.config.memory.collectionName,
                icon: "server.rack",
                color: service.isMemoryConnected ? Colors.primary : Colors.accent
            )
        }
    }
}

@available(macOS 14.0, *)
struct RecentMemoriesSection: View {
    @ObservedObject var service: AppService
    
    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.md) {
            HStack {
                Text("Recent Memories")
                    .font(Typography.title2)
                    .foregroundColor(Colors.textPrimary)
                
                Spacer()

                Text("\(min(service.memories.count, 5)) shown")
                    .font(Typography.caption)
                    .foregroundColor(Colors.primary)
            }
            
            if service.memories.isEmpty {
                EmptyMemoriesPlaceholder {
                    service.toggleCapture()
                }
            } else {
                LazyVStack(spacing: Spacing.md) {
                    ForEach(service.memories.prefix(5)) { memory in
                        CompactMemoryCell(memory: memory)
                    }
                }
            }
        }
    }
}

@available(macOS 14.0, *)
struct EmptyMemoriesPlaceholder: View {
    let onStartCapture: () -> Void
    
    var body: some View {
        VStack(spacing: Spacing.lg) {
            Image(systemName: "brain.head.profile")
                .font(.system(size: 40, weight: .light))
                .foregroundColor(Colors.textTertiary)
            
            Text("No memories yet")
                .font(Typography.title3)
                .foregroundColor(Colors.textPrimary)

            Button {
                onStartCapture()
            } label: {
                Label("Start Capture", systemImage: "record.circle")
            }
                .buttonStyle(PrimaryButtonStyle())
        }
        .frame(maxWidth: .infinity)
        .padding(Spacing.xxxxl)
        .background(Colors.surface)
        .overlay(
            RoundedRectangle(cornerRadius: Radius.xl)
                .stroke(Colors.border, lineWidth: 1)
        )
        .cornerRadius(Radius.xl)
    }
}

@available(macOS 14.0, *)
struct NewMemorySheet: View {
    @ObservedObject var service: AppService
    @State private var content: String = ""
    @Environment(\.dismiss) var dismiss
    
    var body: some View {
        VStack(spacing: Spacing.xl) {
            HStack {
                Text("New Memory")
                    .font(Typography.title2)
                
                Spacer()
                
                Button("Cancel") { dismiss() }
                    .buttonStyle(PlainButtonStyle())
            }
            
            TextEditor(text: $content)
                .frame(minHeight: 120)
                .padding(Spacing.sm)
                .background(Colors.surfaceSecondary)
                .overlay(
                    RoundedRectangle(cornerRadius: Radius.md)
                        .stroke(Colors.border, lineWidth: 1)
                )
                .cornerRadius(Radius.md)
            
            HStack {
                Spacer()
                
                Button("Save") {
                    dismiss()
                }
                .buttonStyle(PrimaryButtonStyle())
                .disabled(content.isEmpty)
                .opacity(content.isEmpty ? 0.5 : 1.0)
            }
        }
        .padding()
        .frame(width: 500)
    }
}

@available(macOS 14.0, *)
struct StatusBadge: View {
    let title: String
    let icon: String
    let color: Color

    var body: some View {
        HStack(spacing: Spacing.xs) {
            Image(systemName: icon)
                .font(.system(size: 12, weight: .semibold))

            Text(title)
                .font(Typography.caption)
                .fontWeight(.medium)
        }
        .foregroundColor(color)
        .padding(.horizontal, Spacing.sm)
        .padding(.vertical, Spacing.xs)
        .background(color.opacity(0.1))
        .overlay(
            RoundedRectangle(cornerRadius: Radius.sm)
                .stroke(color.opacity(0.18), lineWidth: 1)
        )
        .cornerRadius(Radius.sm)
    }
}

@available(macOS 14.0, *)
struct DashboardStatCard: View {
    let title: String
    let value: String
    let detail: String
    let icon: String
    let color: Color

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.lg) {
            HStack {
                Image(systemName: icon)
                    .font(.system(size: 18, weight: .semibold))
                    .foregroundColor(color)
                    .frame(width: 36, height: 36)
                    .background(color.opacity(0.12))
                    .cornerRadius(Radius.md)

                Spacer()
            }

            VStack(alignment: .leading, spacing: Spacing.xs) {
                Text(value)
                    .font(.system(size: 28, weight: .semibold, design: .default))
                    .foregroundColor(Colors.textPrimary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)

                Text(title)
                    .font(Typography.subheadline)
                    .fontWeight(.medium)
                    .foregroundColor(Colors.textSecondary)

                Text(detail)
                    .font(Typography.caption)
                    .foregroundColor(Colors.textTertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
        .frame(maxWidth: .infinity, minHeight: 150, alignment: .leading)
        .padding(Spacing.lg)
        .background(Colors.surface)
        .overlay(
            RoundedRectangle(cornerRadius: Radius.lg)
                .stroke(Colors.border, lineWidth: 1)
        )
        .cornerRadius(Radius.lg)
        .shadow(color: Shadows.sm.color, radius: Shadows.sm.radius, x: Shadows.sm.x, y: Shadows.sm.y)
    }
}
