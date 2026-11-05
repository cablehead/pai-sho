import SwiftUI

/// Add a peer: the invitation (scanned, pasted or typed), a name for the
/// peer, then Accept.
struct AcceptSheet: View {
    let store: PeerStore
    @State var invite: String
    @State private var name = ""
    @State private var busy = false
    @State private var problem: String?
    @Environment(\.dismiss) private var dismiss
    @FocusState private var nameFocused: Bool

    private var handle: String {
        invite.components(separatedBy: .newlines).joined()
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var body: some View {
        NavigationStack {
            Form {
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
        Task {
            do {
                try await store.accept(handle: handle, name: name)
                dismiss()
            } catch {
                problem = error.localizedDescription
            }
            busy = false
        }
    }
}
