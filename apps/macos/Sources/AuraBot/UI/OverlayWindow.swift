import Cocoa
import SwiftUI

@available(macOS 14.0, *)
class OverlayWindow: NSWindow {
    private let overlaySize = NSSize(width: 132, height: 132)
    private let screenInset: CGFloat = 24
    private let model = AuraOverlayModel()
    private var position: OverlayPosition
    private var customOrigin: OverlayOrigin?
    private var showsOverFullScreenApps: Bool
    private var dragStartOrigin: NSPoint?
    private let onOriginChanged: (OverlayOrigin) -> Void
    
    init(
        position: OverlayPosition = .bottomRight,
        customOrigin: OverlayOrigin? = nil,
        showsOverFullScreenApps: Bool = true,
        onOriginChanged: @escaping (OverlayOrigin) -> Void = { _ in }
    ) {
        self.position = position
        self.customOrigin = customOrigin
        self.showsOverFullScreenApps = showsOverFullScreenApps
        self.onOriginChanged = onOriginChanged
        super.init(
            contentRect: NSRect(origin: .zero, size: overlaySize),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        
        setupWindow()
    }
    
    private func setupWindow() {
        level = .screenSaver
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true
        acceptsMouseMovedEvents = true
        ignoresMouseEvents = false
        isReleasedWhenClosed = false
        animationBehavior = .none
        applyCollectionBehavior()
        
        contentView = NSHostingView(
            rootView: AuraOverlayLayer(
                model: model,
                onDragBegan: { [weak self] in self?.beginDrag() },
                onDragChanged: { [weak self] translation in self?.drag(by: translation) },
                onDragEnded: { [weak self] in self?.endDrag() }
            )
        )
    }

    override var canBecomeKey: Bool {
        false
    }

    override var canBecomeMain: Bool {
        false
    }

    func update(
        position: OverlayPosition,
        customOrigin: OverlayOrigin?,
        showsOverFullScreenApps: Bool
    ) {
        self.position = position
        self.customOrigin = customOrigin
        self.showsOverFullScreenApps = showsOverFullScreenApps
        applyCollectionBehavior()
        reposition()
    }

    func update(activity: OverlayActivity) {
        model.activity = activity
    }

    func showPersistent() {
        setContentSize(overlaySize)
        reposition()
        orderFrontRegardless()
    }
    
    func hide() {
        orderOut(nil)
    }

    private func reposition() {
        guard let screen = NSScreen.main ?? NSScreen.screens.first else { return }

        let visibleFrame = screen.visibleFrame

        if let customOrigin {
            let origin = NSPoint(x: CGFloat(customOrigin.x), y: CGFloat(customOrigin.y))
            setFrameOrigin(constrainedOrigin(origin))
            return
        }

        let x = visibleFrame.maxX - overlaySize.width - screenInset
        let y: CGFloat

        switch position {
        case .topRight:
            y = visibleFrame.maxY - overlaySize.height - screenInset
        case .bottomRight:
            y = visibleFrame.minY + screenInset
        }

        setFrameOrigin(NSPoint(x: x, y: y))
    }

    private func applyCollectionBehavior() {
        var behavior: NSWindow.CollectionBehavior = [
            .canJoinAllSpaces,
            .ignoresCycle,
            .stationary
        ]

        if showsOverFullScreenApps {
            behavior.insert(.fullScreenAuxiliary)
        }

        collectionBehavior = behavior
    }

    private func beginDrag() {
        guard dragStartOrigin == nil else { return }

        dragStartOrigin = frame.origin
        model.isDragging = true
        orderFrontRegardless()
    }

    private func drag(by translation: CGSize) {
        if dragStartOrigin == nil {
            beginDrag()
        }

        guard let dragStartOrigin else { return }

        let nextOrigin = NSPoint(
            x: dragStartOrigin.x + translation.width,
            y: dragStartOrigin.y - translation.height
        )
        setFrameOrigin(constrainedOrigin(nextOrigin))
    }

    private func endDrag() {
        model.isDragging = false
        dragStartOrigin = nil

        let origin = OverlayOrigin(x: Double(frame.origin.x), y: Double(frame.origin.y))
        customOrigin = origin
        onOriginChanged(origin)
    }

    private func constrainedOrigin(_ origin: NSPoint) -> NSPoint {
        let candidateFrame = NSRect(origin: origin, size: overlaySize)
        let center = NSPoint(x: candidateFrame.midX, y: candidateFrame.midY)
        let screen = NSScreen.screens.first { $0.visibleFrame.contains(center) }
            ?? NSScreen.screens.first { $0.visibleFrame.intersects(candidateFrame) }
            ?? NSScreen.main
            ?? NSScreen.screens.first

        guard let visibleFrame = screen?.visibleFrame else { return origin }

        return NSPoint(
            x: min(max(origin.x, visibleFrame.minX), visibleFrame.maxX - overlaySize.width),
            y: min(max(origin.y, visibleFrame.minY), visibleFrame.maxY - overlaySize.height)
        )
    }
}

@available(macOS 14.0, *)
private final class AuraOverlayModel: ObservableObject {
    @Published var activity: OverlayActivity = .idle
    @Published var isDragging = false
}

@available(macOS 14.0, *)
private struct AuraOverlayLayer: View {
    @ObservedObject var model: AuraOverlayModel
    let onDragBegan: () -> Void
    let onDragChanged: (CGSize) -> Void
    let onDragEnded: () -> Void

    @State private var pulse = false
    @State private var orbit = false
    @State private var didBeginDrag = false

    private var displayState: AuraOverlayDisplayState {
        model.isDragging ? .dragging : AuraOverlayDisplayState(activity: model.activity)
    }

    var body: some View {
        ZStack {
            Circle()
                .fill(displayState.primary.opacity(0.14))
                .frame(width: 112, height: 112)
                .scaleEffect(displayState.outerScale(pulse: pulse))
                .opacity(pulse ? 0.2 : 0.55)

            Circle()
                .stroke(
                    AngularGradient(
                        colors: displayState.ringColors,
                        center: .center
                    ),
                    lineWidth: 2
                )
                .frame(width: 82, height: 82)
                .rotationEffect(.degrees(orbit ? 360 : 0))

            Circle()
                .fill(.ultraThinMaterial)
                .frame(width: displayState.coreSize, height: displayState.coreSize)
                .overlay(
                    Circle()
                        .stroke(displayState.primary.opacity(0.28), lineWidth: 1)
                )
                .shadow(color: Color.black.opacity(0.14), radius: 18, x: 0, y: 8)

            Image(systemName: displayState.symbolName)
                .font(.system(size: displayState.symbolSize, weight: .semibold))
                .foregroundStyle(displayState.primary)
                .scaleEffect(displayState.iconScale(pulse: pulse))

            ForEach(0..<3) { index in
                Capsule()
                    .fill(displayState.barColor(index: index))
                    .frame(width: 4, height: displayState.barHeight(index: index, pulse: pulse))
                    .offset(x: CGFloat(index - 1) * 12, y: 46)
                    .animation(
                        .easeInOut(duration: displayState.barAnimationDuration)
                            .repeatForever(autoreverses: true)
                            .delay(Double(index) * 0.12),
                        value: pulse
                    )
            }
        }
        .frame(width: 132, height: 132)
        .contentShape(Circle())
        .help(displayState.helpText)
        .gesture(
            DragGesture(minimumDistance: 3)
                .onChanged { value in
                    if !didBeginDrag {
                        didBeginDrag = true
                        onDragBegan()
                    }
                    onDragChanged(value.translation)
                }
                .onEnded { _ in
                    didBeginDrag = false
                    onDragEnded()
                }
        )
        .onAppear {
            pulse = true
            withAnimation(.linear(duration: 5.5).repeatForever(autoreverses: false)) {
                orbit = true
            }
        }
        .animation(.spring(response: 0.32, dampingFraction: 0.74), value: displayState)
    }
}

@available(macOS 14.0, *)
private enum AuraOverlayDisplayState: Equatable {
    case idle
    case listening
    case thinking
    case working
    case error
    case dragging

    init(activity: OverlayActivity) {
        switch activity {
        case .idle:
            self = .idle
        case .listening:
            self = .listening
        case .thinking:
            self = .thinking
        case .working:
            self = .working
        case .error:
            self = .error
        }
    }

    var symbolName: String {
        switch self {
        case .idle:
            return "brain.head.profile"
        case .listening:
            return "waveform"
        case .thinking:
            return "sparkles"
        case .working:
            return "gearshape.2"
        case .error:
            return "exclamationmark.triangle"
        case .dragging:
            return "hand.draw"
        }
    }

    var helpText: String {
        switch self {
        case .idle:
            return "Aura is idle. Drag to move."
        case .listening:
            return "Aura is listening. Drag to move."
        case .thinking:
            return "Aura is thinking. Drag to move."
        case .working:
            return "Aura is working. Drag to move."
        case .error:
            return "Aura hit an error. Drag to move."
        case .dragging:
            return "Moving Aura."
        }
    }

    var primary: Color {
        switch self {
        case .idle:
            return Colors.primary
        case .listening:
            return Colors.secondary
        case .thinking:
            return Colors.accent
        case .working:
            return Colors.warning
        case .error:
            return Colors.danger
        case .dragging:
            return Colors.success
        }
    }

    var ringColors: [Color] {
        switch self {
        case .idle:
            return [Colors.primary, Colors.secondary, Colors.accent, Colors.primary]
        case .listening:
            return [Colors.secondary, Colors.primary, Colors.secondary.opacity(0.65), Colors.secondary]
        case .thinking:
            return [Colors.accent, Colors.warning, Colors.primary, Colors.accent]
        case .working:
            return [Colors.warning, Colors.accent, Colors.secondary, Colors.warning]
        case .error:
            return [Colors.danger, Colors.warning, Colors.danger.opacity(0.65), Colors.danger]
        case .dragging:
            return [Colors.success, Colors.primary, Colors.secondary, Colors.success]
        }
    }

    var coreSize: CGFloat {
        self == .dragging ? 70 : 64
    }

    var symbolSize: CGFloat {
        switch self {
        case .working:
            return 25
        case .dragging:
            return 28
        default:
            return 27
        }
    }

    var barAnimationDuration: Double {
        switch self {
        case .working, .thinking:
            return 0.48
        case .idle:
            return 1.05
        default:
            return 0.8
        }
    }

    func outerScale(pulse: Bool) -> CGFloat {
        switch self {
        case .dragging:
            return pulse ? 1.16 : 1.02
        case .working:
            return pulse ? 1.12 : 0.92
        case .idle:
            return pulse ? 1.04 : 0.9
        default:
            return pulse ? 1.08 : 0.88
        }
    }

    func iconScale(pulse: Bool) -> CGFloat {
        switch self {
        case .thinking, .working:
            return pulse ? 1.08 : 0.96
        case .dragging:
            return 1.08
        default:
            return 1
        }
    }

    func barColor(index: Int) -> Color {
        index == 1 ? ringColors[1] : primary
    }

    func barHeight(index: Int, pulse: Bool) -> CGFloat {
        let expanded = CGFloat(16 + index * 5)
        let collapsed = CGFloat(8 + index * 4)

        switch self {
        case .idle:
            return pulse ? collapsed + 4 : collapsed
        case .dragging:
            return 10
        case .error:
            return index == 1 ? 18 : 10
        default:
            return pulse ? expanded : collapsed
        }
    }
}
