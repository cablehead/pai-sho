import SwiftUI

/// Under the peer list: the version, who Pai Sho is built on, and the
/// licences of everything it includes.
struct Credits: View {
    @State private var showing = false

    var body: some View {
        VStack(spacing: 6) {
            Text(appVersion)
                .accessibilityIdentifier("version")
            Text("Pai Sho is [open source](https://github.com/cablehead/pai-sho) and runs on [iroh](https://github.com/n0-computer/iroh), from [number 0](https://n0.computer).")
                .multilineTextAlignment(.center)
            Button("Acknowledgements") { showing = true }
                .buttonStyle(.borderless)
                .font(.footnote)
                .accessibilityIdentifier("acknowledgements")
        }
        .frame(maxWidth: .infinity)
        .sheet(isPresented: $showing) {
            Acknowledgements()
        }
    }
}

/// Every open-source crate linked into the app, with its licence, from
/// Acknowledgements.txt (apple/tools/acknowledgements.py writes it).
struct Acknowledgements: View {
    @Environment(\.dismiss) private var dismiss

    private let entries: [String] = {
        guard let url = Bundle.main.url(forResource: "Acknowledgements", withExtension: "txt"),
              let text = try? String(contentsOf: url, encoding: .utf8) else {
            return ["Pai Sho is open source: https://github.com/cablehead/pai-sho"]
        }
        return text.components(separatedBy: String(repeating: "=", count: 72) + "\n")
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
    }()

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 16) {
                    ForEach(entries.indices, id: \.self) { i in
                        Text(entries[i])
                            .font(i == 0 ? .body : .caption.monospaced())
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        Divider()
                    }
                }
                .padding()
            }
            .navigationTitle("Acknowledgements")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 520, minHeight: 600)
        #endif
    }
}
