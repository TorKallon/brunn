import SwiftUI

struct ConnectionView: View {
    @EnvironmentObject private var model: AppModel
    @State private var email = ""
    @State private var password = ""
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
                        Eyebrow(text: "Welcome back")
                        Text("Your durable context, directly on iPhone")
                            .font(.largeTitle.bold())
                        Text("Sign in with the same account you use on the web.")
                            .font(.body)
                            .foregroundStyle(.secondary)
                    }

                    VStack(alignment: .leading, spacing: 10) {
                        TextField("Email", text: $email)
                            .textContentType(.username)
                            .keyboardType(.emailAddress)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .padding(14)
                            .background(.background, in: RoundedRectangle(cornerRadius: 8))
                            .overlay {
                                RoundedRectangle(cornerRadius: 8)
                                    .stroke(StraylightTheme.line, lineWidth: 1)
                            }
                            .accessibilityIdentifier("login-email")

                        SecureField("Password", text: $password)
                            .textContentType(.password)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .padding(14)
                            .background(.background, in: RoundedRectangle(cornerRadius: 8))
                            .overlay {
                                RoundedRectangle(cornerRadius: 8)
                                    .stroke(StraylightTheme.line, lineWidth: 1)
                            }
                            .accessibilityIdentifier("login-password")

                        if let message = model.connectionMessage {
                            Text(message)
                                .font(.footnote)
                                .foregroundStyle(StraylightTheme.red)
                                .accessibilityLabel("Connection error: \(message)")
                        }

                        Button {
                            signIn()
                        } label: {
                            HStack {
                                if isConnecting { ProgressView().tint(.white) }
                                Text(isConnecting ? "Signing in…" : "Sign in")
                                    .frame(maxWidth: .infinity)
                            }
                            .frame(minHeight: 44)
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(
                            isConnecting
                                || email.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                || password.isEmpty
                        )

                        Link(
                            "Forgot password?",
                            destination: URL(string: "https://straylight.rourkem.com/forgot-password")!
                        )
                        .frame(maxWidth: .infinity, alignment: .center)
                    }

                    VStack(alignment: .leading, spacing: 8) {
                        Label("Your password is sent only to hosted Straylight and is never stored by the app.", systemImage: "lock.shield")
                        Label("A secure session keeps this iPhone signed in for up to 30 days.", systemImage: "iphone.and.arrow.forward")
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

    private func signIn() {
        guard !isConnecting else { return }
        isConnecting = true
        Task {
            await model.connect(email: email, password: password)
            password = ""
            isConnecting = false
        }
    }
}
