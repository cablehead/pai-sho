import SwiftUI
@preconcurrency import AVFoundation

/// The camera, reading QR codes. Calls `onCode` once with the first code it
/// sees, and `onMessage` when there is no camera to read with.
struct QRScannerView: PlatformViewRepresentable {
    var onMessage: @MainActor (String) -> Void
    var onCode: @MainActor (String) -> Void

    typealias Coordinator = ScannerController

    func makeCoordinator() -> ScannerController { ScannerController() }

    func makeView(context: Context) -> PreviewView {
        let view = PreviewView()
        context.coordinator.start(in: view.previewLayer, onMessage: onMessage, onCode: onCode)
        return view
    }

    func updateView(_ view: PreviewView, context: Context) {}

    static func dismantleView(_ view: PreviewView, coordinator: ScannerController) {
        coordinator.stop()
    }
}

/// A view whose layer is the camera preview.
final class PreviewView: PlatformView {
    #if os(iOS)
    override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }

    override init(frame: CGRect) {
        super.init(frame: frame)
        previewLayer.videoGravity = .resizeAspectFill
    }
    #elseif os(macOS)
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
    }

    override func makeBackingLayer() -> CALayer {
        let layer = AVCaptureVideoPreviewLayer()
        layer.videoGravity = .resizeAspectFill
        return layer
    }
    #endif

    required init?(coder: NSCoder) { nil }

    var previewLayer: AVCaptureVideoPreviewLayer {
        layer as! AVCaptureVideoPreviewLayer
    }
}

@MainActor
final class ScannerController: NSObject, AVCaptureMetadataOutputObjectsDelegate {
    private var session: AVCaptureSession?
    private var output: AVCaptureMetadataOutput?
    private var delivered = false
    private weak var preview: AVCaptureVideoPreviewLayer?
    private var onMessage: (@MainActor (String) -> Void)?
    private var onCode: (@MainActor (String) -> Void)?

    static let noCamera = "There is no camera to scan with."
    static let cameraOff = "The camera is off for Pai Sho. Turn it on in Settings, Privacy & Security, Camera."

    func start(
        in preview: AVCaptureVideoPreviewLayer,
        onMessage: @escaping @MainActor (String) -> Void,
        onCode: @escaping @MainActor (String) -> Void
    ) {
        self.onMessage = onMessage
        self.onCode = onCode
        self.preview = preview
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized:
            configure(preview)
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .video) { granted in
                Task { @MainActor in
                    if granted, let preview = self.preview {
                        self.configure(preview)
                    } else {
                        self.onMessage?(Self.cameraOff)
                    }
                }
            }
        default:
            onMessage(Self.cameraOff)
        }
    }

    func stop() {
        output?.setMetadataObjectsDelegate(nil, queue: nil)
        output = nil
        let session = session
        self.session = nil
        guard let session else { return }
        DispatchQueue.global(qos: .userInitiated).async {
            if session.isRunning { session.stopRunning() }
        }
    }

    private func configure(_ preview: AVCaptureVideoPreviewLayer) {
        guard let device = AVCaptureDevice.default(for: .video) else {
            onMessage?(Self.noCamera)
            return
        }
        let session = AVCaptureSession()
        session.beginConfiguration()
        session.sessionPreset = .high
        guard let input = try? AVCaptureDeviceInput(device: device), session.canAddInput(input) else {
            onMessage?(Self.noCamera)
            return
        }
        session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else {
            onMessage?("This camera cannot read a QR code.")
            return
        }
        session.addOutput(output)
        guard output.availableMetadataObjectTypes.contains(.qr) else {
            onMessage?("This camera cannot read a QR code.")
            return
        }
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]
        session.commitConfiguration()
        preview.session = session
        self.session = session
        self.output = output
        DispatchQueue.global(qos: .userInitiated).async {
            session.startRunning()
        }
    }

    nonisolated func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        let value = metadataObjects.compactMap { $0 as? AVMetadataMachineReadableCodeObject }.first?.stringValue
        guard let value else { return }
        Task { @MainActor in
            guard !self.delivered else { return }
            self.delivered = true
            self.stop()
            self.onCode?(value)
        }
    }
}

/// The Scan sheet: the camera, or why there is none, and Cancel.
struct ScanSheet: View {
    var onCode: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var message: String?

    var body: some View {
        NavigationStack {
            ZStack {
                Color.black.ignoresSafeArea()
                QRScannerView(
                    onMessage: { message = $0 },
                    onCode: { onCode($0.trimmingCharacters(in: .whitespacesAndNewlines)) }
                )
                .ignoresSafeArea()
                if let message {
                    ContentUnavailableView(
                        "No camera",
                        systemImage: "camera",
                        description: Text(message)
                    )
                    .foregroundStyle(.white)
                } else {
                    Text("Point the camera at an invitation's QR code.")
                        .font(.callout)
                        .foregroundStyle(.white)
                        .padding(10)
                        .background(.black.opacity(0.5), in: .rect(cornerRadius: 8))
                        .frame(maxHeight: .infinity, alignment: .bottom)
                        .padding(.bottom, 24)
                }
            }
            .navigationTitle("Scan")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 520, minHeight: 360)
        #endif
    }
}
