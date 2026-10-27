//! The one place HTTP clients are made. reqwest is built without a TLS crypto
//! provider of its own (`rustls-no-provider`), so that rustls uses `ring` and
//! the build needs no cmake; building a client before a provider is installed
//! panics. [`builder`] installs it first, so every client, in the server and
//! in tests, goes through here (clippy.toml refuses the others).

use std::sync::Once;

/// A client builder, with the TLS crypto provider installed.
pub fn builder() -> reqwest::ClientBuilder {
    static PROVIDER: Once = Once::new();
    PROVIDER.call_once(|| {
        // Fails only when a provider is already installed, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
    #[allow(clippy::disallowed_methods)]
    reqwest::Client::builder()
}

/// A client with reqwest's defaults, like `reqwest::Client::new()`. Panics
/// where that does: when the system's TLS setup cannot be loaded.
pub fn client() -> reqwest::Client {
    builder()
        .build()
        .expect("an HTTP client with default settings")
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_client_builds_without_anything_set_up_first() {
        // Run alone, nothing else has installed the provider.
        super::client();
        super::builder().build().unwrap();
    }
}
