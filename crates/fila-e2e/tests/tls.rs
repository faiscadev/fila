mod helpers;

use helpers::TestServer;
use std::sync::Once;

static INIT_CRYPTO: Once = Once::new();

fn ensure_crypto_provider() {
    INIT_CRYPTO.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Generate a self-signed cert/key pair for testing.
///
/// Returns (cert_pem, key_pem). The cert is self-signed, so the cert itself
/// acts as the CA cert when configuring the client.
fn generate_self_signed_cert() -> (Vec<u8>, Vec<u8>) {
    let subject_alt_names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    let cert =
        rcgen::generate_simple_self_signed(subject_alt_names).expect("generate self-signed cert");
    let cert_pem = cert
        .serialize_pem()
        .expect("serialize cert PEM")
        .into_bytes();
    let key_pem = cert.serialize_private_key_pem().into_bytes();
    (cert_pem, key_pem)
}

/// Start fila-server with TLS enabled.
///
/// Returns (TestServer, https_addr, cert_pem) where `cert_pem` is both the
/// server certificate and the CA cert to use for client verification.
fn start_tls_server() -> (TestServer, String, Vec<u8>) {
    use std::net::TcpListener;

    let port = {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().unwrap().port()
    };
    let addr = format!("127.0.0.1:{port}");

    let (cert_pem, key_pem) = generate_self_signed_cert();

    let data_dir = tempfile::tempdir().expect("temp dir");
    let cert_path = data_dir.path().join("server.crt");
    let key_path = data_dir.path().join("server.key");
    std::fs::write(&cert_path, &cert_pem).expect("write cert");
    std::fs::write(&key_path, &key_pem).expect("write key");

    let config_content = format!(
        "[fibp]\nlisten_addr = \"{addr}\"\n\n[tls]\ncert_file = \"{cert}\"\nkey_file = \"{key}\"\n",
        cert = cert_path.to_str().unwrap(),
        key = key_path.to_str().unwrap(),
    );
    let config_path = data_dir.path().join("fila.toml");
    std::fs::write(&config_path, config_content).expect("write config");

    let binary = {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop(); // crates/
        p.pop(); // workspace root
        p.push("target");
        p.push("debug");
        p.push("fila-server");
        p
    };
    assert!(
        binary.exists(),
        "fila-server binary not found at {binary:?}. Run `cargo build` first."
    );

    let child = std::process::Command::new(&binary)
        .env(
            "FILA_DATA_DIR",
            data_dir.path().join("data").to_str().unwrap(),
        )
        .current_dir(data_dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start fila-server with TLS");

    // Poll TCP until the server is reachable.
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(10) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let https_addr = format!("https://{addr}");
    let server = TestServer::from_parts(child, https_addr.clone(), data_dir);
    (server, https_addr, cert_pem)
}

#[tokio::test]
async fn tls_valid_cert_connects_successfully() {
    ensure_crypto_provider();
    let (_server, addr, ca_pem) = start_tls_server();

    // The cert is self-signed, so using the cert itself as the CA cert lets
    // the client verify the server's identity.
    let result = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_tls_ca_cert(ca_pem),
    )
    .await;

    assert!(
        result.is_ok(),
        "expected successful TLS connection, got: {result:?}"
    );
}

#[tokio::test]
async fn tls_plaintext_client_is_rejected() {
    ensure_crypto_provider();
    let (_server, https_addr, _ca_pem) = start_tls_server();

    // Connect using http:// (no TLS) — the server speaks TLS, so this must fail.
    let plaintext_addr = https_addr.replace("https://", "http://");

    // The SDK connect() is lazy by default. We need to actually attempt an RPC
    // to trigger the TLS handshake failure.
    let connect_result = fila_sdk::FilaClient::connect(&plaintext_addr).await;

    match connect_result {
        Err(_) => {
            // connect() itself failed — acceptable
        }
        Ok(client) => {
            // The TLS server will reject a plaintext HTTP/2 client; the RPC must fail.
            let rpc_result = client
                .enqueue("probe-queue", Default::default(), b"x")
                .await;
            assert!(
                rpc_result.is_err(),
                "plaintext client should not be able to make RPCs to a TLS server"
            );
        }
    }
}

#[tokio::test]
async fn tls_wrong_ca_cert_is_rejected() {
    ensure_crypto_provider();
    let (_server, addr, _correct_ca_pem) = start_tls_server();

    // Generate an entirely different cert/key pair; its cert is a different CA.
    let (wrong_ca_pem, _) = generate_self_signed_cert();

    let connect_result = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_tls_ca_cert(wrong_ca_pem),
    )
    .await;

    match connect_result {
        Err(_) => {
            // connect() failed during TLS handshake — expected
        }
        Ok(client) => {
            // TLS may be lazy — the first RPC must fail cert verification.
            let rpc_result = client
                .enqueue("probe-queue", Default::default(), b"x")
                .await;
            assert!(
                rpc_result.is_err(),
                "wrong CA should be rejected during TLS handshake"
            );
        }
    }
}
