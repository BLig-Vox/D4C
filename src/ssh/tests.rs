use super::*;
use russh::{keys::PrivateKey, server};

#[test]
fn managed_public_key_matches_the_saved_identity_and_is_reused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("id_ed25519");
    let public = managed_public_key(&path).unwrap();
    let private = load_secret_key(&path, None).unwrap();
    assert!(public.starts_with("ssh-ed25519 "));
    assert_eq!(public, private.public_key().to_openssh().unwrap());
    assert_eq!(public, managed_public_key(&path).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }
}

struct TestServer;

impl server::Handler for TestServer {
    type Error = russh::Error;

    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        Ok(if user == "tester" && password == "test-password" {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }
}

#[test]
fn verifies_host_before_authenticating() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let key = PrivateKey::from(russh::keys::ssh_key::private::Ed25519Keypair::from_seed(
                &[7; 32],
            ));
            let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
            let config = Arc::new(server::Config {
                keys: vec![key],
                auth_rejection_time: Duration::ZERO,
                ..Default::default()
            });
            for (trust, password, expected) in [
                (false, "test-password", "Host key not accepted."),
                (true, "wrong", "SSH authentication was rejected."),
                (true, "test-password", ""),
            ] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let endpoint = Endpoint {
                    host: "127.0.0.1".into(),
                    port: listener.local_addr().unwrap().port(),
                    user: "tester".into(),
                };
                let config = config.clone();
                let server = tokio::spawn(async move {
                    let (stream, _) = listener.accept().await.unwrap();
                    if let Ok(session) = server::run_stream(config, stream, TestServer).await {
                        let _ = session.await;
                    }
                });
                let (tx, rx) = mpsc::channel();
                let task = tokio::spawn(connect(
                    endpoint,
                    Credential::Password(password.into()),
                    Reporter {
                        tx,
                        ctx: egui::Context::default(),
                    },
                ));
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        if let Ok(Event::Verify {
                            fingerprint: actual,
                            reply,
                        }) = rx.try_recv()
                        {
                            assert_eq!(actual, fingerprint);
                            assert!(!task.is_finished());
                            reply.send(trust).unwrap();
                            break;
                        }
                        assert!(!task.is_finished(), "connection ended before verification");
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    let result = task.await.unwrap();
                    if expected.is_empty() {
                        assert!(!result.unwrap_or_else(|error| panic!("{error}")).is_closed());
                    } else {
                        assert_eq!(result.err().as_deref(), Some(expected));
                    }
                })
                .await
                .unwrap();
                server.abort();
            }
        });
}
