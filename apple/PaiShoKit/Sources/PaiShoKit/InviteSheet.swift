#if os(macOS)
import AppKit
import CoreImage.CIFilterBuiltins
import SwiftUI

/// Invite: asks the daemon for a one-time invitation and shows it as a QR
/// code for a phone to scan, with the text to copy and how long it stays
/// valid. The daemon keeps an invitation for 5 minutes.
struct InviteSheet: View {
    let client: DaemonClient
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var state: Phase = .asking

    enum Phase {
        case asking
        case ready(invite: String, image: NSImage, expires: Date)
        case failed(String)
    }

    static let validity: TimeInterval = 5 * 60

    var body: some View {
        NavigationStack {
            VStack(spacing: 16) {
                switch state {
                case .asking:
                    ProgressView("Asking pai-sho for an invitation...")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                case .ready(let invite, let image, let expires):
                    ready(invite: invite, image: image, expires: expires)
                case .failed(let message):
                    failed(message)
                }
                HStack {
                    TextField("Name", text: $name, prompt: Text("Name the peer (optional)"))
                        .labelsHidden()
                        .onSubmit { Task { await ask() } }
                    Button("New invitation") { Task { await ask() } }
                }
            }
            .padding(20)
            .navigationTitle("Invite")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .frame(minWidth: 460)
        .task { await ask() }
    }

    private func ready(invite: String, image: NSImage, expires: Date) -> some View {
        VStack(spacing: 12) {
            Image(nsImage: image)
                .interpolation(.none)
                .resizable()
                .scaledToFit()
                .frame(width: 280, height: 280)
                .accessibilityLabel("QR code of the invitation")
            Text("Scan this with Pai Sho on the phone.")
                .foregroundStyle(.secondary)
            TimelineView(.periodic(from: .now, by: 1)) { context in
                let left = max(0, Int(expires.timeIntervalSince(context.date).rounded(.up)))
                if left > 0 {
                    Text("Valid for \(left / 60):\(String(format: "%02d", left % 60))")
                        .font(.body.monospacedDigit())
                } else {
                    Text("Expired. Make a new invitation.")
                        .foregroundStyle(.red)
                }
            }
            HStack(alignment: .top) {
                Text(invite)
                    .font(.caption.monospaced())
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Button("Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(invite, forType: .string)
                }
            }
        }
    }

    private func failed(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("pai-sho did not make an invitation.")
                .font(.headline)
            Text(message)
            Text("Run this in a terminal instead. It prints the same QR code:")
            Text("pai-sho invite --qr")
                .font(.body.monospaced())
                .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func ask() async {
        state = .asking
        do {
            let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
            let invite = try await client.invite(name: trimmed.isEmpty ? nil : trimmed)
            guard let image = Self.qrImage(invite) else {
                state = .failed("Could not draw a QR code of the invitation.")
                return
            }
            state = .ready(invite: invite, image: image, expires: Date().addingTimeInterval(Self.validity))
        } catch {
            state = .failed(error.localizedDescription)
        }
    }

    /// `text` as a QR code, one pixel per module plus a quiet zone; the view
    /// scales it up without smoothing.
    static func qrImage(_ text: String) -> NSImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        let rep = NSCIImageRep(ciImage: output)
        let image = NSImage(size: rep.size)
        image.addRepresentation(rep)
        return image
    }
}
#endif
