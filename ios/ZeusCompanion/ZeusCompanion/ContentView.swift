import SwiftUI
import UIKit

// MARK: - Root Content View

struct ContentView: View {
    @ObservedObject var model: CompanionModel
    @State private var showingSessionsDrawer = false
    @State private var showingSettings = false
    @State private var showingSessionInfo = false

    var body: some View {
        ZStack {
            Theme.Colors.background
                .ignoresSafeArea()

            if model.client == nil {
                EditorialPairingView(model: model)
            } else {
                WorkspaceView(
                    model: model,
                    showingSessionsDrawer: $showingSessionsDrawer,
                    showingSettings: $showingSettings,
                    showingSessionInfo: $showingSessionInfo
                )
            }
        }
        .sheet(isPresented: $showingSessionsDrawer) {
            SessionsDrawerSheet(
                model: model,
                showingSettings: $showingSettings
            )
            .presentationDetents([.medium, .large])
            .presentationDragIndicator(.visible)
        }
        .sheet(isPresented: $showingSettings) {
            GatewaySettingsSheet(model: model)
                .presentationDragIndicator(.visible)
        }
        .sheet(isPresented: $showingSessionInfo) {
            if let session = model.selected {
                SessionInfoSheet(session: session, screen: model.screen)
                    .presentationDragIndicator(.visible)
            }
        }
        .alert("Notice", isPresented: .constant(model.error != nil), presenting: model.error) { _ in
            Button("Dismiss", role: .cancel) {
                model.error = nil
            }
        } message: { Text($0) }
    }
}

// MARK: - Primary Workspace View (Full-Screen Conversational AI Workspace)

struct WorkspaceView: View {
    @ObservedObject var model: CompanionModel
    @Binding var showingSessionsDrawer: Bool
    @Binding var showingSettings: Bool
    @Binding var showingSessionInfo: Bool

    @State private var promptText = ""
    @FocusState private var isComposerFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            // Lightweight Contextual Header
            WorkspaceHeader(
                model: model,
                onOpenDrawer: {
                    Haptic.light()
                    showingSessionsDrawer = true
                },
                onOpenSettings: {
                    Haptic.light()
                    showingSettings = true
                },
                onOpenInfo: {
                    Haptic.light()
                    showingSessionInfo = true
                }
            )

            Divider()
                .overlay(Theme.Colors.separator)

            // Conversation / Document Workspace
            if model.showRawTerminal, let screen = model.screen {
                RawTerminalView(screen: screen)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        VStack(alignment: .leading, spacing: 0) {
                            if model.blocks.isEmpty {
                                EmptyWorkspaceView { suggestedPrompt in
                                    promptText = suggestedPrompt
                                    isComposerFocused = true
                                    Haptic.selection()
                                }
                                .padding(.top, Theme.Spacing.xxl)
                            } else {
                                LazyVStack(alignment: .leading, spacing: Theme.Spacing.xl) {
                                    ForEach(model.blocks) { block in
                                        WorkspaceBlockRow(block: block)
                                            .id(block.id)
                                    }

                                    if model.isGenerating {
                                        GeneratingIndicatorRow()
                                            .id("generating_indicator")
                                    }
                                }
                                .padding(.horizontal, Theme.Spacing.lg)
                                .padding(.top, Theme.Spacing.lg)
                                .padding(.bottom, Theme.Spacing.xxl)
                            }
                        }
                    }
                    .onChange(of: model.blocks.count) { _, _ in
                        withAnimation(.easeOut(duration: 0.2)) {
                            if let lastId = model.blocks.last?.id {
                                proxy.scrollTo(lastId, anchor: .bottom)
                            }
                        }
                    }
                }
            }

            // Message Composer Surface
            WorkspaceComposer(
                promptText: $promptText,
                isFocused: $isComposerFocused,
                isGenerating: model.isGenerating,
                onSend: { text in
                    Task {
                        await model.sendPrompt(text)
                    }
                },
                onQuickAction: { actionPrompt in
                    promptText = actionPrompt
                    isComposerFocused = true
                },
                onTakeControl: {
                    Task {
                        await model.acquireControl()
                    }
                }
            )
        }
    }
}

// MARK: - Lightweight Contextual Header

struct WorkspaceHeader: View {
    @ObservedObject var model: CompanionModel
    let onOpenDrawer: () -> Void
    let onOpenSettings: () -> Void
    let onOpenInfo: () -> Void

    var body: some View {
        HStack(alignment: .center, spacing: Theme.Spacing.md) {
            // Leading Menu Button (Opens Sessions Drawer)
            Button(action: onOpenDrawer) {
                Image(systemName: "sidebar.left")
                    .font(.system(size: 17, weight: .regular))
                    .foregroundStyle(Theme.Colors.primaryText)
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }

            Spacer()

            // Center Context / Session Title Indicator
            Button(action: onOpenDrawer) {
                HStack(spacing: 6) {
                    if let session = model.selected {
                        Circle()
                            .fill(session.isLive ? Theme.Colors.accent : Color.secondary.opacity(0.4))
                            .frame(width: 6, height: 6)

                        Text(session.title)
                            .font(Theme.Typography.title)
                            .foregroundStyle(Theme.Colors.primaryText)
                            .lineLimit(1)
                    } else {
                        Text("Zeus Workspace")
                            .font(Theme.Typography.title)
                            .foregroundStyle(Theme.Colors.primaryText)
                    }

                    Image(systemName: "chevron.down")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Theme.Colors.secondaryText)
                }
            }

            Spacer()

            // Trailing Options Menu
            Menu {
                if model.selected != nil {
                    Button(action: onOpenInfo) {
                        Label("Session Details", systemImage: "info.circle")
                    }

                    Button {
                        withAnimation {
                            model.showRawTerminal.toggle()
                        }
                    } label: {
                        Label(
                            model.showRawTerminal ? "View Document Flow" : "View Raw Terminal",
                            systemImage: model.showRawTerminal ? "doc.text" : "terminal"
                        )
                    }

                    if model.control?.hasOwner == true {
                        Button {
                            Task { await model.releaseControl() }
                        } label: {
                            Label("Release Control", systemImage: "lock.open")
                        }
                    } else {
                        Button {
                            Task { await model.acquireControl() }
                        } label: {
                            Label("Take Control", systemImage: "lock")
                        }
                    }

                    Divider()

                    Button {
                        Task { await model.refreshScreen() }
                    } label: {
                        Label("Refresh Screen", systemImage: "arrow.clockwise")
                    }
                }

                Button(action: onOpenSettings) {
                    Label("Gateway Settings", systemImage: "gearshape")
                }
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 17, weight: .regular))
                    .foregroundStyle(Theme.Colors.primaryText)
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
        }
        .padding(.horizontal, Theme.Spacing.sm)
        .frame(height: 52)
        .background(Theme.Colors.background)
    }
}

// MARK: - Empty / New Conversation State

struct EmptyWorkspaceView: View {
    let onSelectPrompt: (String) -> Void

    private let suggestedPrompts = [
        "Check git status",
        "Run cargo test",
        "Show current directory",
        "List running processes"
    ]

    var body: some View {
        VStack(spacing: Theme.Spacing.xxl) {
            Spacer(minLength: 40)

            // Subtle Branding Mark
            VStack(spacing: Theme.Spacing.md) {
                Image(systemName: "bolt.fill")
                    .font(.system(size: 28, weight: .regular))
                    .foregroundStyle(Theme.Colors.primaryText.opacity(0.8))

                Text("How can I help?")
                    .font(Theme.Typography.heading)
                    .foregroundStyle(Theme.Colors.primaryText)
            }

            // Subtle Suggested Prompts
            VStack(spacing: Theme.Spacing.sm) {
                ForEach(suggestedPrompts, id: \.self) { prompt in
                    Button {
                        onSelectPrompt(prompt)
                    } label: {
                        HStack {
                            Text(prompt)
                                .font(Theme.Typography.body)
                                .foregroundStyle(Theme.Colors.primaryText)
                            Spacer()
                            Image(systemName: "arrow.up.right")
                                .font(.system(size: 12))
                                .foregroundStyle(Theme.Colors.secondaryText)
                        }
                        .padding(.horizontal, Theme.Spacing.lg)
                        .padding(.vertical, Theme.Spacing.md)
                        .background(Theme.Colors.surface)
                        .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                                .stroke(Theme.Colors.separator, lineWidth: 1)
                        )
                    }
                }
            }
            .padding(.horizontal, Theme.Spacing.xl)

            Spacer()
        }
        .frame(maxWidth: .infinity)
    }
}

// MARK: - Workspace Block Row (Document Flow)

struct WorkspaceBlockRow: View {
    let block: WorkspaceBlock
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Spacing.sm) {
            switch block.kind {
            case .userPrompt:
                // User message: understated, body typography, no colored bubble
                Text(block.content)
                    .font(Theme.Typography.bodyLarge)
                    .foregroundStyle(Theme.Colors.primaryText)
                    .lineSpacing(4)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.top, Theme.Spacing.sm)

            case .assistantResponse, .systemNote:
                // AI Response / System note: clean typography
                Text(block.content)
                    .font(Theme.Typography.body)
                    .foregroundStyle(Theme.Colors.primaryText)
                    .lineSpacing(4)
                    .frame(maxWidth: .infinity, alignment: .leading)

            case .codeBlock:
                // Code block / Terminal Output: subtle contrasting surface + copy action
                VStack(alignment: .leading, spacing: 0) {
                    // Header
                    HStack {
                        Text(block.title ?? "Output")
                            .font(Theme.Typography.caption)
                            .foregroundStyle(Theme.Colors.secondaryText)

                        Spacer()

                        Button {
                            UIPasteboard.general.string = block.content
                            Haptic.success()
                            withAnimation { copied = true }
                            DispatchQueue.main.asyncAfter(deadline: .now() + 1.6) {
                                withAnimation { copied = false }
                            }
                        } label: {
                            HStack(spacing: 4) {
                                Image(systemName: copied ? "checkmark" : "doc.on.doc")
                                Text(copied ? "Copied" : "Copy")
                            }
                            .font(Theme.Typography.caption)
                            .foregroundStyle(copied ? Theme.Colors.accent : Theme.Colors.secondaryText)
                        }
                    }
                    .padding(.horizontal, Theme.Spacing.md)
                    .padding(.vertical, Theme.Spacing.sm)
                    .background(Theme.Colors.codeSurface)

                    Divider()
                        .overlay(Theme.Colors.separator)

                    // Code / Monospace Content
                    ScrollView(.horizontal, showsIndicators: false) {
                        Text(block.content)
                            .font(Theme.Typography.mono(size: 13))
                            .foregroundStyle(Theme.Colors.primaryText)
                            .textSelection(.enabled)
                            .padding(Theme.Spacing.md)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                .background(Theme.Colors.codeSurface)
                .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                        .stroke(Theme.Colors.separator, lineWidth: 1)
                )
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// MARK: - Generating Indicator Row

struct GeneratingIndicatorRow: View {
    @State private var phase = 0

    var body: some View {
        HStack(spacing: 5) {
            ForEach(0..<3) { index in
                Circle()
                    .fill(Theme.Colors.secondaryText)
                    .frame(width: 5, height: 5)
                    .opacity(phase == index ? 1.0 : 0.3)
            }
            Text("Working...")
                .font(Theme.Typography.caption)
                .foregroundStyle(Theme.Colors.secondaryText)
                .padding(.leading, 4)
        }
        .padding(.vertical, Theme.Spacing.xs)
        .onAppear {
            Timer.scheduledTimer(withTimeInterval: 0.35, repeats: true) { timer in
                phase = (phase + 1) % 3
            }
        }
    }
}

// MARK: - Message Composer (Floating Tactile Surface)

struct WorkspaceComposer: View {
    @Binding var promptText: String
    @FocusState.Binding var isFocused: Bool
    let isGenerating: Bool
    let onSend: (String) -> Void
    let onQuickAction: (String) -> Void
    let onTakeControl: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .bottom, spacing: Theme.Spacing.sm) {
                // Action (+) Button
                Menu {
                    Button {
                        onTakeControl()
                    } label: {
                        Label("Acquire Lease", systemImage: "lock")
                    }

                    Divider()

                    Button {
                        onQuickAction("git status")
                    } label: {
                        Label("git status", systemImage: "arrow.triangle.branch")
                    }

                    Button {
                        onQuickAction("cargo test")
                    } label: {
                        Label("cargo test", systemImage: "play")
                    }

                    Button {
                        onQuickAction("clear")
                    } label: {
                        Label("clear screen", systemImage: "trash")
                    }
                } label: {
                    Image(systemName: "plus")
                        .font(.system(size: 17, weight: .regular))
                        .foregroundStyle(Theme.Colors.secondaryText)
                        .frame(width: 36, height: 36)
                        .contentShape(Circle())
                }

                // Multiline Text Area
                TextField("Ask anything or send command...", text: $promptText, axis: .vertical)
                    .font(Theme.Typography.body)
                    .lineLimit(1...5)
                    .focused($isFocused)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .padding(.vertical, 8)

                // Circular Send Button
                Button {
                    let text = promptText
                    promptText = ""
                    Haptic.medium()
                    onSend(text)
                } label: {
                    ZStack {
                        Circle()
                            .fill(canSend ? Theme.Colors.sendButtonBackground : Theme.Colors.separator)
                            .frame(width: 32, height: 32)

                        Image(systemName: "arrow.up")
                            .font(.system(size: 14, weight: .bold))
                            .foregroundStyle(canSend ? Theme.Colors.sendButtonForeground : Theme.Colors.secondaryText)
                    }
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
                }
                .disabled(!canSend || isGenerating)
            }
            .padding(.horizontal, Theme.Spacing.md)
            .padding(.vertical, 4)
            .background(Theme.Colors.elevatedSurface)
            .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.composer, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.Radius.composer, style: .continuous)
                    .stroke(isFocused ? Theme.Colors.borderFocused : Theme.Colors.separator, lineWidth: 1)
            )
            .padding(.horizontal, Theme.Spacing.lg)
            .padding(.top, Theme.Spacing.xs)
            .padding(.bottom, Theme.Spacing.sm)
        }
        .background(Theme.Colors.background)
    }

    private var canSend: Bool {
        !promptText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
}

// MARK: - Raw Terminal Canvas (Developer Inspection Mode)

struct RawTerminalView: View {
    let screen: Screen

    var body: some View {
        ScrollView([.horizontal, .vertical]) {
            Text(screen.text.isEmpty ? "(waiting for terminal output)" : screen.text)
                .font(Theme.Typography.mono(size: 12))
                .foregroundStyle(Theme.Colors.primaryText)
                .textSelection(.enabled)
                .padding(Theme.Spacing.md)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Theme.Colors.codeSurface)
    }
}

// MARK: - Sessions Drawer Sheet

struct SessionsDrawerSheet: View {
    @ObservedObject var model: CompanionModel
    @Binding var showingSettings: Bool
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Button {
                        model.startNewWorkspace()
                        dismiss()
                        Haptic.light()
                    } label: {
                        HStack(spacing: Theme.Spacing.md) {
                            Image(systemName: "plus")
                                .font(.system(size: 14, weight: .medium))
                                .foregroundStyle(Theme.Colors.primaryText)
                            Text("New Workspace")
                                .font(Theme.Typography.body)
                                .foregroundStyle(Theme.Colors.primaryText)
                            Spacer()
                        }
                    }
                }

                Section("Workspaces") {
                    if model.sessions.isEmpty {
                        Text("No active sessions")
                            .font(Theme.Typography.secondary)
                            .foregroundStyle(Theme.Colors.secondaryText)
                    } else {
                        ForEach(model.sessions) { session in
                            Button {
                                Task {
                                    await model.select(session)
                                }
                                dismiss()
                                Haptic.selection()
                            } label: {
                                HStack(spacing: Theme.Spacing.md) {
                                    Circle()
                                        .fill(session.isLive ? Theme.Colors.accent : Theme.Colors.tertiaryText)
                                        .frame(width: 8, height: 8)

                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(session.title)
                                            .font(Theme.Typography.body)
                                            .foregroundStyle(Theme.Colors.primaryText)
                                            .lineLimit(1)

                                        Text(session.displayCwd)
                                            .font(Theme.Typography.mono(size: 11))
                                            .foregroundStyle(Theme.Colors.secondaryText)
                                            .lineLimit(1)
                                    }

                                    Spacer()

                                    if model.selected?.id == session.id {
                                        Image(systemName: "checkmark")
                                            .font(.system(size: 12, weight: .semibold))
                                            .foregroundStyle(Theme.Colors.accent)
                                    }
                                }
                            }
                        }
                    }
                }

                Section {
                    Button {
                        dismiss()
                        showingSettings = true
                        Haptic.light()
                    } label: {
                        HStack(spacing: Theme.Spacing.md) {
                            Image(systemName: "gearshape")
                                .font(.system(size: 14))
                                .foregroundStyle(Theme.Colors.secondaryText)
                            Text("Gateway Settings")
                                .font(Theme.Typography.body)
                                .foregroundStyle(Theme.Colors.primaryText)
                            Spacer()
                            Text(model.client?.serverID.prefix(8) ?? "")
                                .font(Theme.Typography.mono(size: 11))
                                .foregroundStyle(Theme.Colors.secondaryText)
                        }
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle("Workspaces")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}

// MARK: - Session Info Sheet

struct SessionInfoSheet: View {
    let session: Session
    let screen: Screen?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section("Identity") {
                    LabeledContent("Title", value: session.title)
                    LabeledContent("ID", value: session.id)
                        .font(Theme.Typography.mono(size: 12))
                    LabeledContent("Kind", value: session.kindLabel)
                    LabeledContent("Status", value: session.status.capitalized)
                    LabeledContent("Revision", value: session.revision)
                }

                Section("Working Directory") {
                    Text(session.cwd)
                        .font(Theme.Typography.mono(size: 12))
                        .foregroundStyle(Theme.Colors.secondaryText)
                }

                if let screen {
                    Section("Terminal Details") {
                        LabeledContent("Geometry", value: screen.dimensionsDescription)
                        LabeledContent("Cursor", value: screen.cursorDescription)
                        LabeledContent("Truncated", value: screen.truncated ? "Yes" : "No")
                        LabeledContent("Exited", value: screen.exited ? "Yes" : "No")
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle("Session Details")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}

// MARK: - Gateway Settings Sheet

struct GatewaySettingsSheet: View {
    @ObservedObject var model: CompanionModel
    @Environment(\.dismiss) private var dismiss
    @State private var showingUnpairConfirmation = false

    var body: some View {
        NavigationStack {
            List {
                Section("Gateway Connection") {
                    if let client = model.client {
                        LabeledContent("Server ID", value: client.serverID)
                            .font(Theme.Typography.mono(size: 12))
                        LabeledContent("Origin", value: client.origin.absoluteString)
                            .font(Theme.Typography.mono(size: 12))
                    }
                    LabeledContent("Status", value: model.status)
                    if let hello = model.hello {
                        LabeledContent("API Version", value: "v\(hello.apiMajor).\(hello.apiMinor ?? 0)")
                    }
                }

                Section {
                    Button(role: .destructive) {
                        showingUnpairConfirmation = true
                    } label: {
                        HStack {
                            Spacer()
                            Text("Unpair Device")
                                .foregroundStyle(Theme.Colors.destructive)
                            Spacer()
                        }
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle("Gateway Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .confirmationDialog("Unpair Device?", isPresented: $showingUnpairConfirmation, titleVisibility: .visible) {
                Button("Unpair and Remove Token", role: .destructive) {
                    model.unpair()
                    dismiss()
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("This removes the authentication token from Keychain and disconnects from the Zeus gateway.")
            }
        }
    }
}

// MARK: - Editorial Minimalist Pairing View

struct EditorialPairingView: View {
    @ObservedObject var model: CompanionModel
    @State private var payloadText = ""
    @State private var deviceName = UIDevice.current.name
    @State private var detectedClipboardPayload: String?

    var body: some View {
        ScrollView {
            VStack(spacing: Theme.Spacing.xxl) {
                Spacer(minLength: 48)

                // Minimal Brand Header
                VStack(spacing: Theme.Spacing.md) {
                    Image(systemName: "bolt.fill")
                        .font(.system(size: 32, weight: .regular))
                        .foregroundStyle(Theme.Colors.primaryText)

                    Text("Zeus Companion")
                        .font(Theme.Typography.heading)
                        .foregroundStyle(Theme.Colors.primaryText)

                    Text("Connect to your local or remote development host.")
                        .font(Theme.Typography.secondary)
                        .foregroundStyle(Theme.Colors.secondaryText)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, Theme.Spacing.xl)
                }

                // Clipboard Auto-Detection Card
                if let clipboard = detectedClipboardPayload {
                    VStack(alignment: .leading, spacing: Theme.Spacing.md) {
                        HStack {
                            Text("Pairing payload detected")
                                .font(Theme.Typography.title)
                                .foregroundStyle(Theme.Colors.primaryText)
                            Spacer()
                        }

                        Text("Tap to connect with the configuration found on your clipboard.")
                            .font(Theme.Typography.secondary)
                            .foregroundStyle(Theme.Colors.secondaryText)

                        Button {
                            Haptic.medium()
                            Task {
                                await model.pair(payloadText: clipboard, deviceName: deviceName)
                            }
                        } label: {
                            HStack {
                                if model.isRefreshing {
                                    ProgressView()
                                } else {
                                    Text("Connect Now")
                                        .font(Theme.Typography.title)
                                }
                            }
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, Theme.Spacing.md)
                            .background(Theme.Colors.sendButtonBackground)
                            .foregroundStyle(Theme.Colors.sendButtonForeground)
                            .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
                        }
                        .disabled(model.isRefreshing)
                    }
                    .editorialCard()
                    .padding(.horizontal, Theme.Spacing.lg)
                }

                // Manual Input Section
                VStack(alignment: .leading, spacing: Theme.Spacing.md) {
                    Text("Manual Configuration")
                        .font(Theme.Typography.caption)
                        .foregroundStyle(Theme.Colors.secondaryText)

                    TextEditor(text: $payloadText)
                        .font(Theme.Typography.mono(size: 12))
                        .frame(minHeight: 100)
                        .padding(Theme.Spacing.sm)
                        .background(Theme.Colors.surface)
                        .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                                .stroke(Theme.Colors.separator, lineWidth: 1)
                        )
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()

                    TextField("Device name", text: $deviceName)
                        .font(Theme.Typography.body)
                        .padding(Theme.Spacing.md)
                        .background(Theme.Colors.surface)
                        .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous)
                                .stroke(Theme.Colors.separator, lineWidth: 1)
                        )

                    Button {
                        Haptic.medium()
                        Task {
                            await model.pair(payloadText: payloadText, deviceName: deviceName)
                        }
                    } label: {
                        HStack {
                            if model.isRefreshing {
                                ProgressView()
                            } else {
                                Text("Pair Device")
                                    .font(Theme.Typography.title)
                            }
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, Theme.Spacing.md)
                        .background(payloadText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? Theme.Colors.separator : Theme.Colors.sendButtonBackground)
                        .foregroundStyle(payloadText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? Theme.Colors.secondaryText : Theme.Colors.sendButtonForeground)
                        .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.medium, style: .continuous))
                    }
                    .disabled(payloadText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || model.isRefreshing)
                }
                .editorialCard()
                .padding(.horizontal, Theme.Spacing.lg)

                // Security Reassurance Note
                Text("Encrypted TLS · Hardware Keychain Storage")
                    .font(Theme.Typography.caption)
                    .foregroundStyle(Theme.Colors.tertiaryText)
                    .padding(.bottom, Theme.Spacing.xxl)
            }
        }
        .onAppear {
            checkClipboard()
        }
    }

    private func checkClipboard() {
        guard let clip = UIPasteboard.general.string else { return }
        let trimmed = clip.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.contains("origin") && (trimmed.contains("server_id") || trimmed.contains("serverID")) {
            detectedClipboardPayload = trimmed
            if payloadText.isEmpty {
                payloadText = trimmed
            }
        }
    }
}
