import SwiftUI

struct ConnectionView: View {
    @EnvironmentObject private var model: AppModel
    @State private var token = ""
    @State private var isConnecting = false

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    HStack(spacing: 12) {
                        BrandMark()
                        Text("Straylight")
                            .font(.title2.bold())
                    }

                    VStack(alignment: .leading, spacing: 8) {
                        Eyebrow(text: "Owner alpha")
                        Text("Your durable context, directly on iPhone")
                            .font(.largeTitle.bold())
                        Text("Connect one dedicated device credential. It is validated against hosted Straylight, then kept only in this iPhone's Keychain.")
                            .font(.body)
                            .foregroundStyle(.secondary)
                    }

                    VStack(alignment: .leading, spacing: 10) {
                        SecureField("Dedicated read_only credential", text: $token)
                            .textContentType(.password)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .padding(14)
                            .background(.background, in: RoundedRectangle(cornerRadius: 8))
                            .overlay {
                                RoundedRectangle(cornerRadius: 8)
                                    .stroke(StraylightTheme.line, lineWidth: 1)
                            }

                        if let message = model.connectionMessage {
                            Text(message)
                                .font(.footnote)
                                .foregroundStyle(StraylightTheme.red)
                                .accessibilityLabel("Connection error: \(message)")
                        }

                        Button {
                            isConnecting = true
                            Task {
                                await model.connect(with: token)
                                isConnecting = false
                            }
                        } label: {
                            HStack {
                                if isConnecting { ProgressView().tint(.white) }
                                Text(isConnecting ? "Connecting…" : "Connect this iPhone")
                                    .frame(maxWidth: .infinity)
                            }
                            .frame(minHeight: 44)
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(isConnecting || token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }

                    VStack(alignment: .leading, spacing: 8) {
                        Label("Only a least-privilege read_only device credential is accepted.", systemImage: "key.horizontal")
                        Label("The plaintext is never written to files, logs, or app preferences.", systemImage: "lock.shield")
                        Label("Server-side revocation remains an external owner action in this alpha.", systemImage: "person.badge.key")
                    }
                    .font(.footnote)
                    .foregroundStyle(.secondary)

                    Button("Explore the deterministic demo") {
                        model.enterDemo()
                    }
                    .frame(maxWidth: .infinity)
                    .frame(minHeight: 44)
                }
                .padding(24)
                .frame(maxWidth: 560)
                .frame(maxWidth: .infinity)
            }
            .background(StraylightTheme.canvas)
        }
    }
}
