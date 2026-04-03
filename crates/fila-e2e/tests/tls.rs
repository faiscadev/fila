mod helpers;

use helpers::TestServer;
use std::io::BufRead;

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
/// Returns (TestServer, addr, cert_pem) where `cert_pem` is both the
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
        "[server]\nlisten_addr = \"{addr}\"\n\n[telemetry]\notlp_endpoint = \"\"\n\n[tls]\ncert_file = \"{cert}\"\nkey_file = \"{key}\"\n",
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

    let mut child = std::process::Command::new(&binary)
        .env(
            "FILA_DATA_DIR",
            data_dir.path().join("data").to_str().unwrap(),
        )
        .current_dir(data_dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start fila-server with TLS");

    // Drain stdout/stderr so the child process doesn't block on full pipes.
    let stdout = child.stdout.take().expect("stdout");
    std::thread::spawn(move || for _ in std::io::BufReader::new(stdout).lines() {});
    let stderr = child.stderr.take().expect("stderr");
    std::thread::spawn(move || for _ in std::io::BufReader::new(stderr).lines() {});

    // Poll TCP until the listener is reachable.
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(10) {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        std::net::TcpStream::connect(&addr).is_ok(),
        "TLS fila-server did not become reachable within 10s"
    );

    let server = TestServer::from_parts(child, addr.clone(), data_dir);
    (server, addr, cert_pem)
}

#[tokio::test]
async fn tls_valid_cert_connects_successfully() {
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
    let (_server, addr, _ca_pem) = start_tls_server();

    // Connect without TLS — the server speaks TLS, so the handshake must fail.
    let connect_result = fila_sdk::FilaClient::connect(&addr).await;

    // With the binary protocol, a plaintext connection to a TLS server should
    // fail during the protocol handshake (the server expects a TLS ClientHello).
    assert!(
        connect_result.is_err(),
        "plaintext client should fail to connect to a TLS server"
    );
}

#[tokio::test]
async fn tls_wrong_ca_cert_is_rejected() {
    let (_server, addr, _correct_ca_pem) = start_tls_server();

    // Generate an entirely different cert/key pair; its cert is a different CA.
    let (wrong_ca_pem, _) = generate_self_signed_cert();

    let connect_result = fila_sdk::FilaClient::connect_with_options(
        fila_sdk::ConnectOptions::new(&addr).with_tls_ca_cert(wrong_ca_pem),
    )
    .await;

    // TLS handshake fails because the server's cert is not signed by wrong_ca.
    assert!(
        connect_result.is_err(),
        "wrong CA should be rejected during TLS handshake"
    );
}
