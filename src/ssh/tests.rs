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

struct CodexServer {
    browser_port: u16,
    device: bool,
    signed_in: bool,
    direct_channel: Option<russh::ChannelId>,
    app_channel: Option<russh::ChannelId>,
}

impl CodexServer {
    fn send(
        session: &mut server::Session,
        channel: russh::ChannelId,
        value: Value,
    ) -> Result<(), russh::Error> {
        session.data(channel, format!("{value}\n").into_bytes())
    }

    fn complete(&mut self, session: &mut server::Session) -> Result<(), russh::Error> {
        self.signed_in = true;
        Self::send(
            session,
            self.app_channel.unwrap(),
            json!({
                "method":"account/login/completed", "params":{"loginId":"test-login","success":true}
            }),
        )
    }
}

use serde_json::{Value, json};

impl server::Handler for CodexServer {
    type Error = russh::Error;

    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<server::Auth, russh::Error> {
        TestServer.auth_password(user, password).await
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), russh::Error> {
        self.app_channel = Some(channel.id());
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), russh::Error> {
        assert_eq!(data, b"codex app-server --listen stdio://");
        session.channel_success(channel)
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: russh::Channel<server::Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), russh::Error> {
        assert!(!self.device);
        assert_eq!((host, port), ("127.0.0.1", u32::from(self.browser_port)));
        self.direct_channel = Some(channel.id());
        reply.accept().await;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), russh::Error> {
        if self.direct_channel == Some(channel) {
            assert!(data.starts_with(b"GET /auth/callback?code=fixture&state=fixture-state "));
            session.data(
                channel,
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK".as_slice(),
            )?;
            session.eof(channel)?;
            return self.complete(session);
        }
        for request in data
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let request: Value = serde_json::from_slice(request).unwrap();
            let params = &request["params"];
            let result = match request["method"].as_str().unwrap() {
                "initialize" => json!({}),
                "initialized" => continue,
                "account/read" => {
                    json!({"account":self.signed_in.then(|| json!({"type":"chatgpt","email":"fixture@example.test"})),"requiresOpenaiAuth":true})
                }
                "account/login/start" if self.device => {
                    assert_eq!(params["type"], "chatgptDeviceCode");
                    json!({"loginId":"test-login","verificationUrl":"https://auth.openai.com/codex/device","userCode":"FIXTURE-123"})
                }
                "account/login/start" => {
                    assert_eq!(params["type"], "chatgpt");
                    let mut url =
                        url::Url::parse("https://auth.openai.com/oauth/authorize").unwrap();
                    url.query_pairs_mut()
                        .append_pair(
                            "redirect_uri",
                            &format!("http://127.0.0.1:{}/auth/callback", self.browser_port),
                        )
                        .append_pair("state", "fixture-state");
                    json!({"loginId":"test-login","authUrl":url.as_str()})
                }
                "account/login/cancel" => {
                    assert_eq!(params["loginId"], "test-login");
                    json!({"status":"canceled"})
                }
                "thread/start" => json!({"thread":{"id":"00000000-0000-4000-8000-000000000001"}}),
                "turn/start" => {
                    assert_eq!(params["input"][0]["text"], "hello over SSH");
                    Self::send(
                        session,
                        channel,
                        json!({"id":request["id"],"result":{"turn":{"id":"fixture-turn"}}}),
                    )?;
                    Self::send(
                        session,
                        channel,
                        json!({
                            "method":"item/agentMessage/delta","params":{
                                "threadId":params["threadId"],"turnId":"fixture-turn","itemId":"reply","delta":"fixture response"
                            }
                        }),
                    )?;
                    continue;
                }
                method => panic!("unexpected request: {method}"),
            };
            Self::send(
                session,
                channel,
                json!({"id":request["id"],"result":result}),
            )?;
            if request["method"] == "account/login/start" && self.device {
                self.complete(session)?;
            }
        }
        Ok(())
    }
}

async fn recv<T>(rx: &mpsc::Receiver<T>) -> T {
    loop {
        match rx.try_recv() {
            Ok(value) => return value,
            Err(mpsc::TryRecvError::Empty) => tokio::time::sleep(Duration::from_millis(5)).await,
            Err(error) => panic!("{error}"),
        }
    }
}

#[test]
fn codex_sign_in_and_chat_use_the_selected_ssh_connection() {
    verify_codex_sign_in_and_chat(false, false);
    verify_codex_sign_in_and_chat(true, false);
}

#[test]
fn pasted_redirect_finishes_codex_sign_in_even_when_local_port_is_taken() {
    verify_codex_sign_in_and_chat(false, true);
}

fn verify_codex_sign_in_and_chat(device: bool, paste_redirect: bool) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            tokio::time::timeout(Duration::from_secs(8), async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let endpoint = Endpoint { host: "127.0.0.1".into(), port: listener.local_addr().unwrap().port(), user: "tester".into() };
                let browser_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let browser_port = browser_listener.local_addr().unwrap().port();
                let _occupied = paste_redirect.then_some(browser_listener);
                let config = Arc::new(server::Config {
                    keys: vec![PrivateKey::from(ssh_key::private::Ed25519Keypair::from_seed(&[7;32]))],
                    ..Default::default()
                });
                let server = tokio::spawn(async move {
                    let (stream, _) = listener.accept().await.unwrap();
                    let session = CodexServer { browser_port, device, signed_in: false, direct_channel: None, app_channel: None };
                    server::run_stream(config, stream, session).await.unwrap().await.unwrap();
                });
                let (tx, rx) = mpsc::channel();
                let (commands, mut incoming) = tokio::sync::mpsc::unbounded_channel();
                let task = tokio::spawn(async move {
                    let ctx = egui::Context::default();
                    let connection = connect(endpoint, Credential::Password("test-password".into()), Reporter { tx: tx.clone(), ctx: ctx.clone() }).await.unwrap();
                    codex::serve(&connection, &mut incoming, &Reporter { tx, ctx }).await.unwrap();
                });
                let Event::Verify { reply, .. } = recv(&rx).await else { panic!("expected SSH fingerprint") };
                reply.send(true).unwrap();
                let mut gateway = None;
                loop {
                    match recv(&rx).await {
                        Event::CodexLink(link) => gateway = Some(link),
                        Event::Codex(status) if !status.busy => {
                            assert!(!status.unavailable, "{:?}", status.error);
                            assert!(status.account.is_none());
                            break;
                        }
                        _ => {}
                    }
                }
                commands.send(codex::Command::Login(device)).unwrap();
                let mut saw_login = false;
                loop {
                    match recv(&rx).await {
                        Event::OpenCodexUrl(url) => {
                            assert!(!device);
                            assert!(url.starts_with("https://auth.openai.com/"));
                            let callback = format!("http://127.0.0.1:{browser_port}/auth/callback?code=fixture&state=fixture-state");
                            if paste_redirect {
                                commands.send(codex::Command::SubmitRedirect(callback)).unwrap();
                            } else {
                                let mut browser = tokio::net::TcpStream::connect(("127.0.0.1", browser_port)).await.unwrap();
                                browser.write_all(b"GET /auth/callback?code=fixture&state=fixture-state HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").await.unwrap();
                                let mut response = String::new();
                                browser.read_to_string(&mut response).await.unwrap();
                                assert!(response.ends_with("\r\nOK"), "{response}");
                            }
                            saw_login = true;
                        }
                        Event::Codex(status) => {
                            assert!(status.error.is_none(), "{:?}", status.error);
                            if let Some((url, code)) = status.device {
                                assert!(device);
                                assert_eq!(url, "https://auth.openai.com/codex/device");
                                assert_eq!(code, "FIXTURE-123");
                                saw_login = true;
                            }
                            if let Some(account) = status.account {
                                assert_eq!(account, "fixture@example.test");
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                assert!(saw_login);
                let gateway = gateway.unwrap();
                assert!(matches!(recv(&gateway.rx).await, gateway::Event::Status(status) if status == "Connected"));
                gateway.tx.send(gateway::rpc("chat.send", json!({
                    "sessionKey":"draft:ssh-test","message":"hello over SSH","idempotencyKey":"ssh-test"
                }))).unwrap();
                loop {
                    match recv(&gateway.rx).await {
                        gateway::Event::Data(value) if value["event"] == "chat" => {
                            assert_eq!(value["payload"]["deltaText"], "fixture response");
                            break;
                        }
                        gateway::Event::Error(error) => panic!("{error}"),
                        _ => {}
                    }
                }
                drop(commands);
                task.await.unwrap();
                server.abort();
            }).await.unwrap();
    });
}
