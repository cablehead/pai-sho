import SwiftUI

/// Add a peer: the invitation (scanned, pasted or typed), a name for the
/// peer, then Accept. The sheet waits for the peer's answer. A refused
/// invitation keeps it open, says so, and offers a new one; pai-sho has kept
/// nothing of it.
struct AcceptSheet: View {
    let store: PeerStore
    @State var invite: String
    /// Close this sheet and scan a new invitation (the phone, which has a
    /// camera). Nil where there is no camera to scan with.
    var onScan: (() -> Void)?
    @State private var name = ""
    @State private var busy = false
    @State private var problem: String?
    @State private var refused = false
    @Environment(\.dismiss) private var dismiss
    @FocusState private var nameFocused: Bool

    /// `refused` opens the sheet showing a refusal, as after a dead code.
    init(store: PeerStore, invite: String, refused: Bool = false, onScan: (() -> Void)? = nil) {
        self.store = store
        _invite = State(initialValue: invite)
        _refused = State(initialValue: refused)
        self.onScan = onScan
    }

    private var handle: String {
        invite.components(separatedBy: .newlines).joined()
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// What the peer is called while we wait for it.
    private var joining: String {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? "peer" : trimmed
    }

    var body: some View {
        NavigationStack {
            Form {
                if refused {
                    Section {
                        RefusedNotice(onScan: onScan) { pasted in
                            invite = pasted
                            refused = false
                        }
                    }
                }
                Section("Invitation") {
                    TextField("Invitation", text: $invite, prompt: Text("Paste an invitation"), axis: .vertical)
                        .labelsHidden()
                        .font(.body.monospaced())
                        .lineLimit(3...6)
                        .autocorrectionDisabled()
                        #if os(iOS)
                        .textInputAutocapitalization(.never)
                        #endif
                        .accessibilityIdentifier("invite")
                        .onChange(of: invite) { if !handle.isEmpty { refused = false } }
                }
                Section("Name") {
                    TextField("Name", text: $name, prompt: Text("What to call this peer"))
                        .labelsHidden()
                        .autocorrectionDisabled()
                        #if os(iOS)
                        .textInputAutocapitalization(.never)
                        #endif
                        .focused($nameFocused)
                        .onSubmit(accept)
                        .accessibilityIdentifier("name")
                }
                if busy {
                    Section {
                        HStack(spacing: 8) {
                            ProgressView()
                                .controlSize(.small)
                            Text("Joining \(joining)...")
                        }
                    } footer: {
                        Text("Waiting for it to answer.")
                    }
                }
                if let problem {
                    Section {
                        Text(problem).foregroundStyle(.red)
                    }
                }
            }
            .formStyle(.grouped)
            .disabled(busy)
            .navigationTitle("Add a peer")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .disabled(busy)
                }
                ToolbarItem(placement: .confirmationAction) {
                    if busy {
                        ProgressView()
                    } else {
                        Button("Accept", action: accept)
                            .disabled(handle.isEmpty)
                    }
                }
            }
            .onAppear { nameFocused = !handle.isEmpty }
        }
        #if os(macOS)
        .frame(minWidth: 440, minHeight: 300)
        #endif
    }

    private func accept() {
        guard !busy, !handle.isEmpty else { return }
        busy = true
        problem = nil
        refused = false
        Task {
            do {
                switch try await store.accept(handle: handle, name: name) {
                case .refused:
                    // The code is spent or expired. Clear it, so Accept waits
                    // for a new one.
                    invite = ""
                    refused = true
                case .joined, .pending, .sent:
                    dismiss()
                }
            } catch {
                problem = error.localizedDescription
            }
            busy = false
        }
    }
}

/// What a refused invitation shows: that it was refused, what to do, and the
/// ways to get a new one.
struct RefusedNotice: View {
    var onScan: (() -> Void)?
    var onPaste: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(AcceptOutcome.refusedMessage)
                .font(.headline)
                .foregroundStyle(.red)
            Text("Ask for a new one. An invitation works once, for 5 minutes.")
                .font(.callout)
                .foregroundStyle(.secondary)
            HStack(spacing: 10) {
                if let onScan {
                    Button("Scan a new one", action: onScan)
                        .buttonStyle(.borderedProminent)
                }
                PasteButton(payloadType: String.self) { strings in
                    let text = (strings.first ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
                    Task { @MainActor in onPaste(text) }
                }
            }
        }
        .padding(.vertical, 4)
        .accessibilityIdentifier("refused")
    }
}
