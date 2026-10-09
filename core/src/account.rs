//! Native-only account credentials. Only safe account metadata crosses IPC.
//! Session and refresh operations are serialized, including logout, so a late
//! device poll cannot restore credentials after the user signs out.
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

pub const SUPABASE_URL: &str = "https://akdqlmsktfkxzhvttgpi.supabase.co";
pub const SUPABASE_PUBLISHABLE_KEY: &str = "sb_publishable_n-afxWzNnmCed1gjjehR7Q_CmLgA50u";

pub fn account_url() -> String {
    std::env::var("COMRADE_ACCOUNT_URL")
        .ok()
        .filter(|url| base_url(url).is_ok())
        .unwrap_or_else(|| "https://www.roviumlabs.me/products/comrade".into())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountUser {
    pub id: String,
    pub email: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountStatus {
    pub authenticated: bool,
    pub user: Option<AccountUser>,
    pub pending: bool,
    pub usage_mode: &'static str,
    pub account_url: String,
    pub browser_login_available: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceLogin {
    pub user_code: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

// Deliberately no Debug: credentials must never appear in diagnostics.
#[derive(Clone, Serialize, Deserialize)]
struct Session {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    user: AccountUser,
    server_url: String,
    supabase_url: String,
}

#[derive(Deserialize)]
struct DeviceResponse {
    device_code: String,
    user_code: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    expires_at: u64,
    #[serde(default)]
    expires_in: u64,
    user: AccountUser,
}

struct Pending {
    device_code: String,
    server_url: String,
    expires: Instant,
    interval: Duration,
    next_poll: Instant,
    generation: u64,
}

struct AccountManager {
    path: PathBuf,
    supabase_url: String,
    publishable_key: String,
    client: reqwest::Client,
    pending: Option<Pending>,
    verified: Option<(String, AccountUser, Instant)>,
    cancellation: Arc<AtomicU64>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Accept cleartext only for development on the loopback interface. In
/// particular, reject URL credentials, query strings, and fragments.
fn base_url(raw: &str) -> Result<String> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|_| anyhow!("ACCOUNT_CONFIG: invalid server URL"))?;
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("ACCOUNT_CONFIG: use HTTPS or a local development server URL");
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

impl AccountManager {
    fn new(path: PathBuf, supabase_url: &str, publishable_key: &str) -> Result<Self> {
        Ok(Self {
            path,
            supabase_url: base_url(supabase_url)?,
            publishable_key: publishable_key.into(),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(20))
                .build()
                .map_err(|_| anyhow!("ACCOUNT_NETWORK: could not initialize account client"))?,
            pending: None,
            verified: None,
            cancellation: Arc::new(AtomicU64::new(0)),
        })
    }

    fn read(&self) -> Result<Option<Session>> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => bail!("ACCOUNT_STORAGE: could not read account session"),
        };
        let session: Session = serde_json::from_slice(&bytes).map_err(|_| {
            anyhow!("ACCOUNT_STORAGE: account session is damaged; sign out and sign in again")
        })?;
        if session.supabase_url != self.supabase_url {
            bail!("ACCOUNT_LOGIN_REQUIRED: account belongs to a different identity service");
        }
        Ok(Some(session))
    }

    fn write(&self, session: &Session) -> Result<()> {
        use std::io::Write;
        let parent = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)
            .map_err(|_| anyhow!("ACCOUNT_STORAGE: could not create account directory"))?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp = parent.join(format!(".auth-session-{}-{stamp}.tmp", std::process::id()));
        let result = (|| -> Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&serde_json::to_vec(session)?)?;
            file.sync_all()?;
            // Windows cannot atomically replace an existing file with rename.
            #[cfg(windows)]
            if self.path.exists() {
                std::fs::remove_file(&self.path)?;
            }
            std::fs::rename(&temp, &self.path)?;
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&temp);
        result.map_err(|_| anyhow!("ACCOUNT_STORAGE: could not securely save account session"))
    }

    fn clear(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(anyhow!("ACCOUNT_STORAGE: could not remove account session")),
        }
    }

    fn pending(&mut self) -> bool {
        if self.pending.as_ref().is_some_and(|p| {
            Instant::now() >= p.expires || p.generation != self.cancellation.load(Ordering::SeqCst)
        }) {
            self.pending = None;
        }
        self.pending.is_some()
    }

    fn state(&mut self, user: Option<AccountUser>) -> AccountStatus {
        AccountStatus {
            authenticated: user.is_some(),
            user,
            pending: self.pending(),
            usage_mode: "unlimited",
            account_url: account_url(),
            browser_login_available: std::env::var("COMRADE_BROWSER_LOGIN_ENABLED")
                .is_ok_and(|value| value.eq_ignore_ascii_case("true")),
        }
    }

    async fn start(&mut self, server_url: &str) -> Result<DeviceLogin> {
        self.pending = None;
        let server_url = base_url(server_url)?;
        let generation = self.cancellation.load(Ordering::SeqCst);
        let response = self
            .client
            .post(format!("{server_url}/v1/auth/device"))
            .send()
            .await
            .map_err(|_| anyhow!("ACCOUNT_NETWORK: unable to start sign in"))?;
        if !response.status().is_success() {
            bail!(
                "ACCOUNT_UNAVAILABLE: sign in service returned {}",
                response.status().as_u16()
            );
        }
        let response: DeviceResponse = response
            .json()
            .await
            .map_err(|_| anyhow!("ACCOUNT_PROTOCOL: invalid sign in response"))?;
        if response.device_code.is_empty()
            || response.user_code.is_empty()
            || response.expires_in == 0
        {
            bail!("ACCOUNT_PROTOCOL: incomplete sign in response");
        }
        // This is opened by the native shell. Never allow custom/file schemes.
        let _ = base_url(
            &response
                .verification_uri_complete
                .split('?')
                .next()
                .unwrap_or(""),
        )?;
        let interval = response.interval.clamp(1, 30);
        let expires_in = response.expires_in.min(900);
        if generation != self.cancellation.load(Ordering::SeqCst) {
            bail!("ACCOUNT_CANCELLED: sign in was cancelled");
        }
        self.pending = Some(Pending {
            device_code: response.device_code,
            server_url,
            expires: Instant::now() + Duration::from_secs(expires_in),
            interval: Duration::from_secs(interval),
            next_poll: Instant::now(),
            generation,
        });
        Ok(DeviceLogin {
            user_code: response.user_code,
            verification_uri_complete: response.verification_uri_complete,
            expires_in,
            interval,
        })
    }

    async fn validate(&mut self, server_url: &str) -> Result<Option<Session>> {
        let Some(mut session) = self.read()? else {
            self.verified = None;
            return Ok(None);
        };
        if session.server_url != base_url(server_url)? {
            bail!("ACCOUNT_LOGIN_REQUIRED: sign in again for the selected server");
        }
        if session.access_token.is_empty() || session.refresh_token.is_empty() {
            self.verified = None;
            self.clear()?;
            return Ok(None);
        }
        if session.expires_at <= now().saturating_add(60) {
            self.verified = None;
            let response = self
                .client
                .post(format!(
                    "{}/auth/v1/token?grant_type=refresh_token",
                    self.supabase_url
                ))
                .header("apikey", &self.publishable_key)
                .json(&serde_json::json!({ "refresh_token": session.refresh_token }))
                .send()
                .await
                .map_err(|_| {
                    anyhow!("ACCOUNT_NETWORK: unable to refresh sign in; retry when connected")
                })?;
            if matches!(response.status().as_u16(), 400 | 401 | 403) {
                self.clear()?;
                return Ok(None);
            }
            if !response.status().is_success() {
                bail!("ACCOUNT_UNAVAILABLE: unable to refresh sign in; retry later");
            }
            let tokens: TokenResponse = response
                .json()
                .await
                .map_err(|_| anyhow!("ACCOUNT_PROTOCOL: invalid account refresh response"))?;
            session = self.session(tokens, &session.server_url)?;
            // Persist rotated refresh credentials before any subsequent network
            // request, so a transient user lookup cannot lose the refresh token.
            self.write(&session)?;
        }
        let response = self
            .client
            .get(format!("{}/auth/v1/user", self.supabase_url))
            .header("apikey", &self.publishable_key)
            .bearer_auth(&session.access_token)
            .send()
            .await
            .map_err(|_| {
                anyhow!("ACCOUNT_NETWORK: unable to verify sign in; retry when connected")
            })?;
        if matches!(response.status().as_u16(), 401 | 403) {
            self.verified = None;
            self.clear()?;
            return Ok(None);
        }
        if !response.status().is_success() {
            bail!("ACCOUNT_UNAVAILABLE: unable to verify sign in; retry later");
        }
        let user: AccountUser = response
            .json()
            .await
            .map_err(|_| anyhow!("ACCOUNT_PROTOCOL: invalid account verification response"))?;
        if user.id.is_empty() || user.id != session.user.id {
            self.verified = None;
            self.clear()?;
            return Ok(None);
        }
        if session.user != user {
            session.user = user;
            self.write(&session)?;
        }
        self.verified = Some((
            session.access_token.clone(),
            session.user.clone(),
            Instant::now(),
        ));
        Ok(Some(session))
    }

    async fn validate_cached(&mut self, server_url: &str) -> Result<Option<Session>> {
        if let Some(session) = self.read()? {
            if session.server_url != base_url(server_url)? {
                bail!("ACCOUNT_LOGIN_REQUIRED: sign in again for the selected server");
            }
            if session.expires_at > now().saturating_add(60)
                && self.verified.as_ref().is_some_and(|(token, user, at)| {
                    token == &session.access_token
                        && user == &session.user
                        && at.elapsed() < Duration::from_secs(60)
                })
            {
                return Ok(Some(session));
            }
        }
        self.validate(server_url).await
    }

    async fn sign_in(
        &mut self,
        email: &str,
        password: &str,
        server_url: &str,
    ) -> Result<AccountStatus> {
        let server_url = base_url(server_url)?;
        let generation = self.cancellation.load(Ordering::SeqCst);
        if email.trim().is_empty() || password.is_empty() {
            bail!("ACCOUNT_AUTH: email and password are required");
        }
        let response = self
            .client
            .post(format!(
                "{}/auth/v1/token?grant_type=password",
                self.supabase_url
            ))
            .header("apikey", &self.publishable_key)
            .json(&serde_json::json!({ "email": email.trim(), "password": password }))
            .send()
            .await
            .map_err(|_| anyhow!("ACCOUNT_NETWORK: unable to sign in; retry when connected"))?;
        if matches!(response.status().as_u16(), 400 | 401 | 403 | 422) {
            bail!("ACCOUNT_AUTH: sign in failed; check your email and password and confirm your email");
        }
        if !response.status().is_success() {
            bail!("ACCOUNT_UNAVAILABLE: unable to sign in; retry later");
        }
        let tokens = response
            .json()
            .await
            .map_err(|_| anyhow!("ACCOUNT_PROTOCOL: invalid sign in response"))?;
        if generation != self.cancellation.load(Ordering::SeqCst) {
            bail!("ACCOUNT_CANCELLED: sign in was cancelled");
        }
        let session = self.session(tokens, &server_url)?;
        self.write(&session)?;
        self.pending = None;
        self.verified = None;
        self.status(&server_url).await
    }

    fn session(&self, tokens: TokenResponse, server_url: &str) -> Result<Session> {
        let expires_at = if tokens.expires_at > 0 {
            tokens.expires_at
        } else {
            now().saturating_add(tokens.expires_in)
        };
        if tokens.access_token.is_empty()
            || tokens.refresh_token.is_empty()
            || tokens.user.id.is_empty()
            || expires_at <= now()
        {
            bail!("ACCOUNT_PROTOCOL: incomplete account credentials");
        }
        Ok(Session {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at,
            user: tokens.user,
            server_url: server_url.into(),
            supabase_url: self.supabase_url.clone(),
        })
    }

    async fn status(&mut self, server_url: &str) -> Result<AccountStatus> {
        let session = self.validate(server_url).await?;
        Ok(self.state(session.map(|s| s.user)))
    }

    async fn poll(&mut self, server_url: &str) -> Result<AccountStatus> {
        if !self.pending() {
            return self.status(server_url).await;
        }
        let pending = self.pending.as_mut().unwrap();
        if pending.server_url != base_url(server_url)? {
            self.pending = None;
            bail!("ACCOUNT_LOGIN_REQUIRED: selected server changed; start sign in again");
        }
        if Instant::now() < pending.next_poll {
            return Ok(self.state(None));
        }
        pending.next_poll = Instant::now() + pending.interval;
        let bound_server = pending.server_url.clone();
        let response = self
            .client
            .post(format!("{bound_server}/v1/auth/device/token"))
            .json(&serde_json::json!({ "device_code": pending.device_code }))
            .send()
            .await
            .map_err(|_| {
                anyhow!("ACCOUNT_NETWORK: unable to check sign in; retry when connected")
            })?;
        if !self.pending() {
            return Ok(self.state(None));
        }
        match response.status().as_u16() {
            202 => return Ok(self.state(None)),
            410 => {
                self.pending = None;
                return Ok(self.state(None));
            }
            200 => {}
            400 | 401 | 403 | 404 => {
                self.pending = None;
                bail!("ACCOUNT_LOGIN_REQUIRED: sign in request was rejected; start again");
            }
            _ => bail!("ACCOUNT_UNAVAILABLE: unable to check sign in; retry later"),
        }
        let tokens: TokenResponse = response
            .json()
            .await
            .map_err(|_| anyhow!("ACCOUNT_PROTOCOL: invalid sign in token response"))?;
        if !self.pending() {
            return Ok(self.state(None));
        }
        let session = self.session(tokens, &bound_server)?;
        self.write(&session)?;
        self.pending = None;
        self.status(server_url).await
    }

    async fn logout(&mut self) -> Result<()> {
        self.pending = None;
        self.verified = None;
        let session = self.read().ok().flatten();
        // Erase local credentials first, even when the auth service is offline.
        self.clear()?;
        if let Some(session) = session {
            let _ = self
                .client
                .post(format!("{}/auth/v1/logout?scope=local", self.supabase_url))
                .header("apikey", &self.publishable_key)
                .bearer_auth(&session.access_token)
                .send()
                .await;
        }
        Ok(())
    }
}

static MANAGER: OnceLock<Mutex<Option<AccountManager>>> = OnceLock::new();
static CANCELLATION: OnceLock<Arc<AtomicU64>> = OnceLock::new();

fn cancel_pending_request() {
    CANCELLATION
        .get_or_init(|| Arc::new(AtomicU64::new(0)))
        .fetch_add(1, Ordering::SeqCst);
}

async fn manager() -> Result<tokio::sync::MutexGuard<'static, Option<AccountManager>>> {
    let mut guard = MANAGER.get_or_init(|| Mutex::new(None)).lock().await;
    let path = crate::paths::comrade_home().join("auth-session.json");
    if guard.as_ref().is_none_or(|manager| manager.path != path) {
        *guard = Some(AccountManager::new(
            path,
            SUPABASE_URL,
            SUPABASE_PUBLISHABLE_KEY,
        )?);
        guard.as_mut().unwrap().cancellation = CANCELLATION
            .get_or_init(|| Arc::new(AtomicU64::new(0)))
            .clone();
    }
    Ok(guard)
}

fn configured_server() -> String {
    crate::config::load_config(&crate::config::find_project_root()).server_url
}

pub async fn start_login() -> Result<DeviceLogin> {
    cancel_pending_request();
    manager()
        .await?
        .as_mut()
        .unwrap()
        .start(&configured_server())
        .await
}

pub async fn poll_login() -> Result<AccountStatus> {
    manager()
        .await?
        .as_mut()
        .unwrap()
        .poll(&configured_server())
        .await
}

pub async fn status() -> Result<AccountStatus> {
    manager()
        .await?
        .as_mut()
        .unwrap()
        .status(&configured_server())
        .await
}

/// Validate the account with the identity service; a file or opaque token is
/// never sufficient proof that the account can use the app.
pub async fn require_user() -> Result<AccountUser> {
    manager()
        .await?
        .as_mut()
        .unwrap()
        .validate_cached(&configured_server())
        .await?
        .map(|s| s.user)
        .ok_or_else(|| {
            anyhow!("ACCOUNT_LOGIN_REQUIRED: create an account or sign in to use Comrade")
        })
}

/// Only the server involved in device login may receive this credential.
pub async fn bearer_for(server_url: &str) -> Result<String> {
    manager()
        .await?
        .as_mut()
        .unwrap()
        .validate_cached(server_url)
        .await?
        .map(|s| s.access_token)
        .ok_or_else(|| anyhow!("ACCOUNT_LOGIN_REQUIRED: sign in to use Comrade"))
}

pub async fn logout() -> Result<()> {
    cancel_pending_request();
    manager().await?.as_mut().unwrap().logout().await
}

pub async fn cancel_login() -> Result<()> {
    cancel_pending_request();
    manager().await?.as_mut().unwrap().pending = None;
    Ok(())
}

pub async fn sign_in(email: &str, password: &str) -> Result<AccountStatus> {
    cancel_pending_request();
    manager()
        .await?
        .as_mut()
        .unwrap()
        .sign_in(email, password, &configured_server())
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex as StdMutex;

    struct Fixture {
        url: String,
        requests: Arc<StdMutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Fixture {
        fn new(replies: Vec<(u16, serde_json::Value)>) -> Self {
            Self::delayed(
                replies
                    .into_iter()
                    .map(|(status, body)| (status, body, 0))
                    .collect(),
            )
        }

        fn delayed(replies: Vec<(u16, serde_json::Value, u64)>) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let requests = Arc::new(StdMutex::new(Vec::new()));
            let captured = requests.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = stop.clone();
            let thread = std::thread::spawn(move || {
                let mut replies = replies.into_iter();
                while !stopping.load(Ordering::SeqCst) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(stream) => stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(_) => return,
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut bytes = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                        }
                        let raw = String::from_utf8_lossy(&bytes);
                        if let Some(end) = raw.find("\r\n\r\n") {
                            let length: usize = raw[..end]
                                .lines()
                                .find_map(|line| {
                                    line.to_lowercase()
                                        .strip_prefix("content-length:")
                                        .and_then(|value| value.trim().parse().ok())
                                })
                                .unwrap_or(0);
                            if bytes.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    captured
                        .lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&bytes).into_owned());
                    let (status, body, delay) =
                        replies.next().unwrap_or((500, serde_json::json!({}), 0));
                    if delay > 0 {
                        std::thread::sleep(Duration::from_millis(delay));
                    }
                    let body = body.to_string();
                    let reply = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(reply.as_bytes());
                }
            });
            Self {
                url,
                requests,
                stop,
                thread: Some(thread),
            }
        }

        fn count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.thread.take().unwrap().join().unwrap();
        }
    }

    struct TestHome(PathBuf);
    impl TestHome {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(
                std::env::temp_dir()
                    .join(format!("comrade-account-{}-{stamp}", std::process::id())),
            )
        }
        fn path(&self) -> PathBuf {
            self.0.join("auth-session.json")
        }
        fn manager(&self, fixture: &Fixture) -> AccountManager {
            AccountManager::new(self.path(), &fixture.url, "publishable-test").unwrap()
        }
    }
    impl Drop for TestHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn user() -> serde_json::Value {
        serde_json::json!({ "id": "user-1", "email": "tester@example.com" })
    }
    fn tokens() -> serde_json::Value {
        serde_json::json!({ "access_token": "access-new", "refresh_token": "refresh-new", "expires_at": now() + 3600, "user": user() })
    }
    fn saved(manager: &AccountManager, server: &str, expired: bool) {
        manager
            .write(&Session {
                access_token: "access-old".into(),
                refresh_token: "refresh-old".into(),
                expires_at: if expired { 1 } else { now() + 3600 },
                user: serde_json::from_value(user()).unwrap(),
                server_url: server.into(),
                supabase_url: manager.supabase_url.clone(),
            })
            .unwrap();
    }

    #[tokio::test]
    async fn missing_session_requires_sign_in_without_network() {
        let fixture = Fixture::new(vec![]);
        let home = TestHome::new();
        let mut manager = home.manager(&fixture);
        let status = manager.status(&fixture.url).await.unwrap();
        assert!(!status.authenticated && !status.pending);
        assert_eq!(status.usage_mode, "unlimited");
        assert_eq!(fixture.count(), 0);
    }

    #[tokio::test]
    async fn refresh_rotates_credentials_validates_identity_and_saves_privately() {
        let fixture = Fixture::new(vec![(200, tokens()), (200, user())]);
        let home = TestHome::new();
        let mut manager = home.manager(&fixture);
        saved(&manager, &fixture.url, true);
        assert!(manager.status(&fixture.url).await.unwrap().authenticated);
        let session = manager.read().unwrap().unwrap();
        assert_eq!(session.refresh_token, "refresh-new");
        let requests = fixture.requests.lock().unwrap();
        assert!(requests[0].starts_with("POST /auth/v1/token?grant_type=refresh_token "));
        assert!(requests[0].contains("refresh-old"));
        assert!(requests[1]
            .to_lowercase()
            .contains("authorization: bearer access-new"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(home.path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn rejected_identity_and_refresh_remove_session() {
        for expired in [false, true] {
            let fixture = Fixture::new(vec![(401, serde_json::json!({}))]);
            let home = TestHome::new();
            let mut manager = home.manager(&fixture);
            saved(&manager, &fixture.url, expired);
            assert!(!manager.status(&fixture.url).await.unwrap().authenticated);
            assert!(!home.path().exists());
        }
    }

    #[tokio::test]
    async fn transient_failure_preserves_existing_or_rotated_session() {
        for expired in [false, true] {
            let fixture = Fixture::new(if expired {
                vec![(200, tokens()), (503, serde_json::json!({}))]
            } else {
                vec![(503, serde_json::json!({}))]
            });
            let home = TestHome::new();
            let mut manager = home.manager(&fixture);
            saved(&manager, &fixture.url, expired);
            let error = manager.status(&fixture.url).await.unwrap_err().to_string();
            assert!(error.starts_with("ACCOUNT_UNAVAILABLE:"));
            let session = manager.read().unwrap().unwrap();
            assert_eq!(
                session.refresh_token,
                if expired {
                    "refresh-new"
                } else {
                    "refresh-old"
                }
            );
        }
    }

    #[tokio::test]
    async fn server_binding_blocks_credentials_before_any_request() {
        let fixture = Fixture::new(vec![]);
        let home = TestHome::new();
        let mut manager = home.manager(&fixture);
        saved(&manager, "https://trusted.example/comrade", false);
        for target in [
            "https://evil.example",
            "https://trusted.example/other",
            "https://trusted.example/comrade?x=1",
        ] {
            assert!(manager.validate_cached(target).await.is_err());
        }
        assert_eq!(fixture.count(), 0);
        assert!(home.path().exists());
    }

    #[tokio::test]
    async fn logout_erases_session_even_when_revoke_unavailable() {
        let fixture = Fixture::new(vec![(503, serde_json::json!({}))]);
        let home = TestHome::new();
        let mut manager = home.manager(&fixture);
        saved(&manager, &fixture.url, false);
        manager.logout().await.unwrap();
        assert!(!home.path().exists());
        let requests = fixture.requests.lock().unwrap();
        assert!(requests[0].starts_with("POST /auth/v1/logout?scope=local "));
        assert!(requests[0]
            .to_lowercase()
            .contains("authorization: bearer access-old"));
    }

    #[tokio::test]
    async fn password_login_and_restart_verify_user_without_exposing_tokens() {
        let fixture = Fixture::new(vec![(200, tokens()), (200, user()), (200, user())]);
        let home = TestHome::new();
        let mut manager = home.manager(&fixture);
        let status = manager
            .sign_in("tester@example.com", "example-password", &fixture.url)
            .await
            .unwrap();
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(status.authenticated);
        assert!(!serialized.contains("access-new") && !serialized.contains("refresh-new"));
        drop(manager);
        let mut restarted = home.manager(&fixture);
        assert!(restarted
            .validate_cached(&fixture.url)
            .await
            .unwrap()
            .is_some());
        assert_eq!(fixture.count(), 3);
        assert!(fixture.requests.lock().unwrap()[0]
            .starts_with("POST /auth/v1/token?grant_type=password "));
    }

    #[tokio::test]
    async fn concurrent_requests_refresh_once_and_cache_verified_identity() {
        let fixture = Fixture::new(vec![(200, tokens()), (200, user())]);
        let home = TestHome::new();
        let manager = home.manager(&fixture);
        saved(&manager, &fixture.url, true);
        let manager = Arc::new(Mutex::new(manager));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let manager = manager.clone();
            let server = fixture.url.clone();
            tasks.push(tokio::spawn(async move {
                manager
                    .lock()
                    .await
                    .validate_cached(&server)
                    .await
                    .unwrap()
                    .is_some()
            }));
        }
        for task in tasks {
            assert!(task.await.unwrap());
        }
        assert_eq!(fixture.count(), 2);
    }

    #[tokio::test]
    async fn status_rechecks_identity_and_does_not_trust_file_presence() {
        let fixture = Fixture::new(vec![(200, user()), (401, serde_json::json!({}))]);
        let home = TestHome::new();
        let mut manager = home.manager(&fixture);
        saved(&manager, &fixture.url, false);
        assert!(manager.status(&fixture.url).await.unwrap().authenticated);
        assert!(manager
            .validate_cached(&fixture.url)
            .await
            .unwrap()
            .is_some());
        assert!(!manager.status(&fixture.url).await.unwrap().authenticated);
        assert!(manager
            .validate_cached(&fixture.url)
            .await
            .unwrap()
            .is_none());
        assert_eq!(fixture.count(), 2);
    }

    #[tokio::test]
    async fn device_pairing_keeps_device_code_private_and_throttles_polling() {
        let fixture = Fixture::new(vec![
            (
                200,
                serde_json::json!({ "device_code": "private-device-code", "user_code": "ABCD-EFGH", "verification_uri_complete": "https://www.roviumlabs.me/products/comrade?code=ABCD-EFGH", "expires_in": 600, "interval": 3 }),
            ),
            (202, serde_json::json!({})),
            (200, tokens()),
            (200, user()),
        ]);
        let home = TestHome::new();
        let mut manager = home.manager(&fixture);
        let login = manager.start(&fixture.url).await.unwrap();
        assert!(!serde_json::to_string(&login)
            .unwrap()
            .contains("private-device-code"));
        assert!(manager.poll(&fixture.url).await.unwrap().pending);
        assert!(manager.poll(&fixture.url).await.unwrap().pending);
        assert_eq!(fixture.count(), 2);
        manager.pending.as_mut().unwrap().next_poll = Instant::now();
        assert!(manager.poll(&fixture.url).await.unwrap().authenticated);
        assert_eq!(fixture.count(), 4);
        assert!(!manager.pending());
    }

    #[tokio::test]
    async fn local_expiry_and_remote_expiry_clear_pending_request() {
        for local in [false, true] {
            let fixture = Fixture::new(vec![
                (
                    200,
                    serde_json::json!({ "device_code": "private", "user_code": "ABCD-EFGH", "verification_uri_complete": "https://www.roviumlabs.me/products/comrade", "expires_in": 600, "interval": 3 }),
                ),
                (410, serde_json::json!({})),
            ]);
            let home = TestHome::new();
            let mut manager = home.manager(&fixture);
            manager.start(&fixture.url).await.unwrap();
            if local {
                manager.pending.as_mut().unwrap().expires = Instant::now();
            }
            let status = manager.poll(&fixture.url).await.unwrap();
            assert!(!status.authenticated && !status.pending);
            assert_eq!(fixture.count(), if local { 1 } else { 2 });
        }
    }

    #[tokio::test]
    async fn cancelled_inflight_device_poll_cannot_install_session() {
        let fixture = Fixture::delayed(vec![
            (
                200,
                serde_json::json!({ "device_code": "private", "user_code": "ABCD-EFGH", "verification_uri_complete": "https://www.roviumlabs.me/products/comrade", "expires_in": 600, "interval": 3 }),
                0,
            ),
            (200, tokens(), 150),
        ]);
        let home = TestHome::new();
        let mut manager = home.manager(&fixture);
        manager.start(&fixture.url).await.unwrap();
        let cancellation = manager.cancellation.clone();
        let server = fixture.url.clone();
        let task = tokio::spawn(async move { manager.poll(&server).await.unwrap() });
        let deadline = Instant::now() + Duration::from_secs(2);
        while fixture.count() < 2 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(fixture.count(), 2);
        cancellation.fetch_add(1, Ordering::SeqCst);
        assert!(!task.await.unwrap().authenticated);
        assert!(!home.path().exists());
    }

    #[test]
    fn urls_require_secure_origin_and_exclude_credentials() {
        assert_eq!(
            base_url("https://example.com/base/").unwrap(),
            "https://example.com/base"
        );
        assert!(base_url("http://127.0.0.1:8080").is_ok());
        for url in [
            "file:///etc/passwd",
            "http://remote.example",
            "https://user:pass@example.com",
            "https://example.com/#fragment",
            "https://example.com/?key=secret",
        ] {
            assert!(base_url(url).is_err());
        }
    }
}
