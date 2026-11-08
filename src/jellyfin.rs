//! The HTTP client for the Jellyfin the launcher started: readiness, sessions,
//! first-time setup and this install's API key. Written against what Jellyfin
//! 12.4.0 was observed to do (reference.md); the fake server in the tests below
//! mimics exactly that, so change the two together.

use std::collections::HashSet;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::decide::{Item, Library, Recent, Scan, Session, User, encode_query};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// A key Jellyfin no longer accepts: revoked in the Dashboard, or the data
    /// folder was replaced. The launcher then asks for the admin password once.
    Unauthorized,
    /// The admin name or password was wrong.
    BadPassword,
    /// Any other HTTP status, by call.
    Status { call: String, status: u16 },
    /// No answer, a timeout, or a body that did not parse.
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unauthorized => write!(f, "Jellyfin rejected the API key"),
            Error::BadPassword => write!(f, "Jellyfin did not accept that name or password"),
            Error::Status { call, status } => write!(f, "Jellyfin {call}: HTTP {status}"),
            Error::Other(e) => write!(f, "Jellyfin: {e}"),
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

/// One Jellyfin. `device` names this machine in Jellyfin's Dashboard.
pub struct Client {
    pub base: String,
    pub device: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(base: &str, device: &str) -> Client {
        Self::with_timeout(base, device, Duration::from_secs(5))
    }

    pub fn with_timeout(base: &str, device: &str, timeout: Duration) -> Client {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .into();
        Client { base: base.trim_end_matches('/').into(), device: device.into(), agent }
    }

    /// Jellyfin's own scheme, and the only one 12.4 accepts for an API key:
    /// `X-Emby-Token` answers 401 there.
    fn authorization(&self, token: Option<&str>) -> String {
        let mut h = format!(
            r#"MediaBrowser Client="{1}", Device="{0}", DeviceId="{2}-{0}", Version="{3}""#,
            self.device,
            crate::decide::APP_NAME,
            crate::decide::APP_ID,
            env!("CARGO_PKG_VERSION")
        );
        if let Some(t) = token.filter(|t| !t.is_empty()) {
            h.push_str(&format!(r#", Token="{t}""#));
        }
        h
    }

    /// One call: (status, body).
    fn call(&self, method: &str, path: &str, body: Option<&serde_json::Value>, token: Option<&str>) -> Result<(u16, String)> {
        let url = format!("{}{path}", self.base);
        let auth = self.authorization(token);
        let resp = match (method, body) {
            ("GET", _) => self.agent.get(&url).header("Authorization", &auth).call(),
            (_, Some(b)) => self
                .agent
                .post(&url)
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .send(b.to_string()),
            ("DELETE", _) => self.agent.delete(&url).header("Authorization", &auth).call(),
            (_, None) => self.agent.post(&url).header("Authorization", &auth).send_empty(),
        };
        let mut resp = resp.map_err(|e| Error::Other(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        Ok((status, text))
    }

    fn expect(&self, method: &str, path: &str, body: Option<&serde_json::Value>, token: Option<&str>, want: u16) -> Result<String> {
        let (status, text) = self.call(method, path, body, token)?;
        if status != want {
            let call = format!("{method} {}", path.split('?').next().unwrap_or(path));
            return Err(Error::Status { call, status });
        }
        Ok(text)
    }

    /// Whether Jellyfin is ready. 12.4 answers /health with 200 and the body
    /// "Degraded" while still starting, so ready means the body says Healthy.
    pub fn healthy(&self) -> bool {
        matches!(self.call("GET", "/health", None, None), Ok((200, body)) if body.trim() == "Healthy")
    }

    /// Every session active in the last 16 minutes.
    pub fn sessions(&self, key: &str) -> Result<Vec<Session>> {
        let (status, text) = self.call("GET", "/Sessions?activeWithinSeconds=960", None, Some(key))?;
        match status {
            200 => {}
            401 | 403 => return Err(Error::Unauthorized),
            s => return Err(Error::Status { call: "GET /Sessions".into(), status: s }),
        }
        let docs: Vec<SessionDoc> = serde_json::from_str(&text).map_err(|e| Error::Other(e.to_string()))?;
        Ok(docs
            .into_iter()
            .map(|d| Session {
                device: d.device_name,
                client: d.client,
                item: d.now_playing_item.map(|i| Item { name: i.name, runtime_ticks: i.run_time_ticks }),
                position_ticks: d.play_state.position_ticks,
                paused: d.play_state.is_paused,
            })
            .collect())
    }

    /// Whether the first-run wizard is still open. The public info needs no key.
    pub fn needs_setup(&self) -> Result<bool> {
        #[derive(Deserialize)]
        struct Info {
            #[serde(rename = "StartupWizardCompleted", default)]
            done: bool,
        }
        let text = self.expect("GET", "/System/Info/Public", None, None, 200)?;
        let info: Info = serde_json::from_str(&text).map_err(|e| Error::Other(e.to_string()))?;
        Ok(!info.done)
    }

    /// The first-run wizard: language, the administrator, the media folder as one
    /// library, remote access OFF, then complete.
    ///
    /// Two things 12.4 requires: `GET /Startup/User` before the POST (the GET
    /// creates the first user, and the POST is a 404 until it has), and the
    /// library added before Complete, while the wizard still allows it without a
    /// key.
    pub fn setup(&self, user: &str, password: &str, media: &str) -> Result<()> {
        let (user, media) = (user.trim(), media.trim());
        if user.is_empty() || password.is_empty() {
            return Err(Error::Other("enter an administrator name and password".into()));
        }
        let post = |path: &str, body: Option<serde_json::Value>| self.expect("POST", path, body.as_ref(), None, 204).map(drop);
        post(
            "/Startup/Configuration",
            Some(json!({"UICulture": "en-US", "MetadataCountryCode": "US", "PreferredMetadataLanguage": "en"})),
        )?;
        self.expect("GET", "/Startup/User", None, None, 200)?;
        post("/Startup/User", Some(json!({"Name": user, "Password": password})))?;
        if !media.is_empty() {
            post(
                "/Library/VirtualFolders?name=Media&collectionType=mixed&refreshLibrary=true",
                Some(json!({"LibraryOptions": {"PathInfos": [{"Path": media}]}})),
            )?;
        }
        post("/Startup/RemoteAccess", Some(json!({"EnableRemoteAccess": false, "EnableAutomaticPortMapping": false})))?;
        post("/Startup/Complete", None)
    }

    /// The key's app name, so the Dashboard's API Keys page shows which install
    /// holds which key.
    pub fn app_name(&self) -> String {
        format!("{}-{}", crate::decide::APP_ID, self.device)
    }

    /// Signs in once, creates this install's key and reads it back, then logs
    /// the session out. The password goes no further than this call.
    ///
    /// POST /Auth/Keys answers 204 with no body, and Jellyfin keeps a second key
    /// under the same app name, so the new key is the one that was not in the
    /// list taken just before: matching by name alone could return a stale one.
    pub fn make_key(&self, user: &str, password: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct SignIn {
            #[serde(rename = "AccessToken")]
            token: String,
        }
        let (status, text) = self.call(
            "POST",
            "/Users/AuthenticateByName",
            Some(&json!({"Username": user.trim(), "Pw": password})),
            None,
        )?;
        match status {
            200 => {}
            401 => return Err(Error::BadPassword),
            s => return Err(Error::Status { call: "POST /Users/AuthenticateByName".into(), status: s }),
        }
        let session = serde_json::from_str::<SignIn>(&text).map_err(|e| Error::Other(e.to_string()))?.token;
        let result = self.create_key(&session);
        let _ = self.expect("POST", "/Sessions/Logout", None, Some(&session), 204);
        result
    }

    fn create_key(&self, session: &str) -> Result<String> {
        let list = |s: &Self| -> Result<KeyList> {
            let text = s.expect("GET", "/Auth/Keys", None, Some(session), 200)?;
            serde_json::from_str(&text).map_err(|e| Error::Other(e.to_string()))
        };
        let known: HashSet<String> = list(self)?.items.into_iter().map(|k| k.access_token).collect();
        let app = self.app_name();
        self.expect("POST", &format!("/Auth/Keys?app={app}"), None, Some(session), 204)?;
        list(self)?
            .items
            .into_iter()
            .find(|k| k.app_name == app && !k.access_token.is_empty() && !known.contains(&k.access_token))
            .map(|k| k.access_token)
            .ok_or_else(|| Error::Other("the key was made but not listed".into()))
    }
}

impl Client {
    /// Every user, administrators first.
    pub fn users(&self, key: &str) -> Result<Vec<User>> {
        #[derive(Deserialize)]
        struct Policy {
            #[serde(rename = "IsAdministrator", default)]
            admin: bool,
        }
        #[derive(Deserialize)]
        struct Doc {
            #[serde(rename = "Id", default)]
            id: String,
            #[serde(rename = "Name", default)]
            name: String,
            #[serde(rename = "HasPassword", default)]
            has_password: bool,
            #[serde(rename = "Policy")]
            policy: Option<Policy>,
        }
        let (status, text) = self.call("GET", "/Users", None, Some(key))?;
        match status {
            200 => {}
            401 | 403 => return Err(Error::Unauthorized),
            s => return Err(Error::Status { call: "GET /Users".into(), status: s }),
        }
        let docs: Vec<Doc> = serde_json::from_str(&text).map_err(|e| Error::Other(e.to_string()))?;
        let mut users: Vec<User> = docs
            .into_iter()
            .map(|d| User { id: d.id, name: d.name, admin: d.policy.is_some_and(|p| p.admin), has_password: d.has_password })
            .collect();
        users.sort_by(|a, b| b.admin.cmp(&a.admin).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(users)
    }

    /// Every library and its folders.
    pub fn libraries(&self, key: &str) -> Result<Vec<Library>> {
        #[derive(Deserialize)]
        struct Doc {
            #[serde(rename = "Name", default)]
            name: String,
            #[serde(rename = "Locations", default)]
            locations: Vec<String>,
        }
        let docs: Vec<Doc> = self.get_json("/Library/VirtualFolders", key)?;
        Ok(docs.into_iter().map(|d| Library { name: d.name, paths: d.locations }).collect())
    }

    /// Adds a library with one folder, and scans it.
    pub fn add_library(&self, key: &str, name: &str, kind: &str, folder: &str) -> Result<()> {
        let path = format!(
            "/Library/VirtualFolders?name={}&collectionType={}&refreshLibrary=true",
            encode_query(name),
            encode_query(kind)
        );
        self.change("POST", &path, Some(json!({"LibraryOptions": {"PathInfos": [{"Path": folder}]}})), key)
    }

    /// A change answered 204; a rejected key is its own error.
    fn change(&self, method: &str, path: &str, body: Option<serde_json::Value>, key: &str) -> Result<()> {
        match self.call(method, path, body.as_ref(), Some(key))?.0 {
            204 => Ok(()),
            401 | 403 => Err(Error::Unauthorized),
            s => Err(Error::Status { call: format!("{method} {}", path.split('?').next().unwrap_or(path)), status: s }),
        }
    }

    /// Adds a folder to a library, and scans it. A folder that does not exist is
    /// a 404 (checked on 12.4).
    pub fn add_folder(&self, key: &str, library: &str, folder: &str) -> Result<()> {
        let body = json!({"Name": library, "PathInfo": {"Path": folder}});
        self.change("POST", "/Library/VirtualFolders/Paths?refreshLibrary=true", Some(body), key)
    }

    /// Removes a folder from a library. The files stay where they are.
    pub fn remove_folder(&self, key: &str, library: &str, folder: &str) -> Result<()> {
        let path = format!(
            "/Library/VirtualFolders/Paths?name={}&path={}&refreshLibrary=true",
            encode_query(library),
            encode_query(folder)
        );
        self.change("DELETE", &path, None, key)
    }

    /// Removes a library. The files stay where they are.
    pub fn remove_library(&self, key: &str, library: &str) -> Result<()> {
        let path = format!("/Library/VirtualFolders?name={}&refreshLibrary=true", encode_query(library));
        self.change("DELETE", &path, None, key)
    }

    /// The newest items, newest first. 12.4 answers 400 without a user id, so
    /// the administrator's is passed.
    pub fn recently_added(&self, key: &str, user_id: &str) -> Result<Vec<Recent>> {
        #[derive(Deserialize)]
        struct Doc {
            #[serde(rename = "Name", default)]
            name: String,
            #[serde(rename = "ProductionYear")]
            year: Option<u32>,
        }
        let docs: Vec<Doc> = self.get_json(&format!("/Items/Latest?userId={}&limit=8", encode_query(user_id)), key)?;
        Ok(docs.into_iter().map(|d| Recent { name: d.name, year: d.year }).collect())
    }

    /// Starts a scan of every library (204; checked on 12.4).
    pub fn scan(&self, key: &str) -> Result<()> {
        match self.call("POST", "/Library/Refresh", None, Some(key))?.0 {
            204 => Ok(()),
            401 | 403 => Err(Error::Unauthorized),
            s => Err(Error::Status { call: "POST /Library/Refresh".into(), status: s }),
        }
    }

    /// The "Scan Media Library" task's state, from the scheduled tasks: its key
    /// is RefreshLibrary, its State "Running" or "Idle" (checked on 12.4).
    pub fn scan_state(&self, key: &str) -> Result<Scan> {
        #[derive(Deserialize)]
        struct Last {
            #[serde(rename = "Status")]
            status: Option<String>,
        }
        #[derive(Deserialize)]
        struct Task {
            #[serde(rename = "Key", default)]
            key: String,
            #[serde(rename = "State", default)]
            state: String,
            #[serde(rename = "CurrentProgressPercentage")]
            percent: Option<f64>,
            #[serde(rename = "LastExecutionResult")]
            last: Option<Last>,
        }
        let tasks: Vec<Task> = self.get_json("/ScheduledTasks?isHidden=false", key)?;
        Ok(tasks
            .into_iter()
            .find(|t| t.key == "RefreshLibrary")
            .map(|t| Scan { running: t.state == "Running", percent: t.percent, last: t.last.and_then(|l| l.status) })
            .unwrap_or_default())
    }

    /// A GET with the key, its body parsed; a rejected key is its own error.
    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str, key: &str) -> Result<T> {
        let (status, text) = self.call("GET", path, None, Some(key))?;
        match status {
            200 => serde_json::from_str(&text).map_err(|e| Error::Other(e.to_string())),
            401 | 403 => Err(Error::Unauthorized),
            s => Err(Error::Status { call: format!("GET {}", path.split('?').next().unwrap_or(path)), status: s }),
        }
    }

    /// Adds a user, never an administrator (checked on 12.4), with a password or
    /// none.
    pub fn add_user(&self, key: &str, name: &str, password: &str) -> Result<User> {
        #[derive(Deserialize)]
        struct Doc {
            #[serde(rename = "Id", default)]
            id: String,
            #[serde(rename = "Name", default)]
            name: String,
        }
        let (status, text) = self.call("POST", "/Users/New", Some(&json!({"Name": name, "Password": password})), Some(key))?;
        match status {
            200 => {}
            401 | 403 => return Err(Error::Unauthorized),
            s => return Err(Error::Status { call: "POST /Users/New".into(), status: s }),
        }
        let d: Doc = serde_json::from_str(&text).map_err(|e| Error::Other(e.to_string()))?;
        Ok(User { id: d.id, name: d.name, admin: false, has_password: !password.is_empty() })
    }

    /// Sets a user's password with the admin key; no current password is needed
    /// (checked on 12.4). Blank removes it. Jellyfin signs that user out on every
    /// device, so a TV signed in as them must sign in again; the key survives.
    pub fn set_password(&self, key: &str, user_id: &str, password: &str) -> Result<()> {
        // The id goes into the path, so only Jellyfin's own form is accepted.
        if user_id.is_empty() || !user_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(Error::Other("not a Jellyfin user id".into()));
        }
        let path = format!("/Users/{user_id}/Password");
        let (status, _) = self.call("POST", &path, Some(&json!({"NewPw": password})), Some(key))?;
        match status {
            204 => Ok(()),
            401 | 403 => Err(Error::Unauthorized),
            s => Err(Error::Status { call: "POST /Users/{id}/Password".into(), status: s }),
        }
    }
}

#[derive(Deserialize)]
struct SessionDoc {
    #[serde(rename = "DeviceName", default)]
    device_name: String,
    #[serde(rename = "Client", default)]
    client: String,
    #[serde(rename = "NowPlayingItem")]
    now_playing_item: Option<ItemDoc>,
    #[serde(rename = "PlayState", default)]
    play_state: PlayStateDoc,
}

#[derive(Deserialize)]
struct ItemDoc {
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "RunTimeTicks", default)]
    run_time_ticks: i64,
}

#[derive(Deserialize, Default)]
struct PlayStateDoc {
    #[serde(rename = "PositionTicks", default)]
    position_ticks: i64,
    #[serde(rename = "IsPaused", default)]
    is_paused: bool,
}

#[derive(Deserialize)]
struct KeyList {
    #[serde(rename = "Items", default)]
    items: Vec<KeyDoc>,
}

#[derive(Deserialize)]
struct KeyDoc {
    #[serde(rename = "AccessToken", default)]
    access_token: String,
    #[serde(rename = "AppName", default)]
    app_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::{Handler, Request, serve};
    use std::sync::{Arc, Mutex};

    /// Jellyfin 12.4.0 as observed on a real server.
    #[derive(Default)]
    struct Fake {
        complete: bool,
        user_made: bool,
        user: String,
        pw: String,
        library: String,
        remote: Option<bool>,
        keys: Vec<(String, String)>, // (app, token)
        sessions: HashSet<String>,
        logged_out: Vec<String>,
        /// (id, name, admin, password)
        users: Vec<(String, String, bool, String)>,
        libraries: Vec<(String, Vec<String>)>,
        scanning: bool,
        calls: Vec<String>,
        n: u32,
    }

    fn token(r: &Request) -> String {
        r.header("Authorization")
            .and_then(|h| h.split("Token=\"").nth(1))
            .and_then(|t| t.split('"').next())
            .unwrap_or("")
            .to_string()
    }

    fn fake(state: Arc<Mutex<Fake>>) -> Client {
        let handler: Handler = Arc::new(move |r: &Request| {
            let mut f = state.lock().unwrap();
            f.calls.push(format!("{} {}", r.method, r.path()));
            let body: serde_json::Value = serde_json::from_str(&r.body).unwrap_or_default();
            if r.path().starts_with("/Startup/") && f.complete {
                return (401, None);
            }
            match (r.method.as_str(), r.path()) {
                ("GET", "/System/Info/Public") => (200, Some(format!(r#"{{"Version":"12.4.0","StartupWizardCompleted":{}}}"#, f.complete))),
                ("POST", "/Startup/Configuration") => (204, None),
                ("GET", "/Startup/User") => {
                    f.user_made = true;
                    (200, Some(r#"{"Name":"user"}"#.into()))
                }
                ("POST", "/Startup/User") if !f.user_made => (404, None),
                ("POST", "/Startup/User") => {
                    f.user = body["Name"].as_str().unwrap().into();
                    f.pw = body["Password"].as_str().unwrap().into();
                    (204, None)
                }
                ("POST", "/Library/VirtualFolders") if f.complete && !f.keys.iter().any(|(_, k)| *k == token(r)) => (401, None),
                ("POST", "/Library/VirtualFolders") if !f.complete => {
                    f.library = format!("{}={}", r.query("name").unwrap(), body["LibraryOptions"]["PathInfos"][0]["Path"].as_str().unwrap());
                    (204, None)
                }
                ("POST", "/Startup/RemoteAccess") => {
                    f.remote = body["EnableRemoteAccess"].as_bool();
                    (204, None)
                }
                ("POST", "/Startup/Complete") => {
                    f.complete = true;
                    (204, None)
                }
                ("POST", "/Users/AuthenticateByName") => {
                    if body["Username"] != f.user.as_str() || body["Pw"] != f.pw.as_str() {
                        return (401, Some("Error processing request.".into()));
                    }
                    f.n += 1;
                    let t = format!("session-{}", f.n);
                    f.sessions.insert(t.clone());
                    (200, Some(format!(r#"{{"User":{{}},"AccessToken":"{t}"}}"#)))
                }
                ("POST", "/Auth/Keys") if f.sessions.contains(&token(r)) => {
                    f.n += 1;
                    let k = format!("key-{}", f.n);
                    f.keys.push((r.query("app").unwrap(), k));
                    (204, None)
                }
                ("GET", "/Auth/Keys") if f.sessions.contains(&token(r)) => {
                    let items: Vec<_> = f.keys.iter().map(|(a, t)| json!({"AppName": a, "AccessToken": t})).collect();
                    (200, Some(json!({"Items": items}).to_string()))
                }
                ("POST", "/Sessions/Logout") => {
                    let t = token(r);
                    f.sessions.remove(&t);
                    f.logged_out.push(t);
                    (204, None)
                }
                ("GET", "/Sessions") if r.header("X-Emby-Token").is_some() => (401, None),
                ("GET", "/Library/VirtualFolders") if f.keys.iter().any(|(_, k)| *k == token(r)) => {
                    let items: Vec<_> = f.libraries.iter().map(|(n, p)| json!({"Name": n, "Locations": p, "CollectionType": "movies"})).collect();
                    (200, Some(json!(items).to_string()))
                }
                ("POST", "/Library/VirtualFolders") if f.keys.iter().any(|(_, k)| *k == token(r)) => {
                    let name = r.query("name").unwrap();
                    f.calls.push(format!("kind={} refresh={}", r.query("collectionType").unwrap(), r.query("refreshLibrary").unwrap()));
                    let path = body["LibraryOptions"]["PathInfos"][0]["Path"].as_str().unwrap().to_string();
                    f.libraries.push((name, vec![path]));
                    (204, None)
                }
                ("POST", "/Library/VirtualFolders/Paths") if f.keys.iter().any(|(_, k)| *k == token(r)) => {
                    let (name, path) = (body["Name"].as_str().unwrap().to_string(), body["PathInfo"]["Path"].as_str().unwrap().to_string());
                    if path.contains("missing") {
                        return (404, Some("Error processing request.".into()));
                    }
                    match f.libraries.iter_mut().find(|l| l.0 == name) {
                        Some(l) => {
                            l.1.push(path);
                            (204, None)
                        }
                        None => (404, None),
                    }
                }
                ("DELETE", "/Library/VirtualFolders/Paths") if f.keys.iter().any(|(_, k)| *k == token(r)) => {
                    let (name, path) = (r.query("name").unwrap(), r.query("path").unwrap());
                    match f.libraries.iter_mut().find(|l| l.0 == name) {
                        Some(l) => {
                            l.1.retain(|p| *p != path);
                            (204, None)
                        }
                        None => (404, None),
                    }
                }
                ("DELETE", "/Library/VirtualFolders") if f.keys.iter().any(|(_, k)| *k == token(r)) => {
                    let name = r.query("name").unwrap();
                    let before = f.libraries.len();
                    f.libraries.retain(|l| l.0 != name);
                    if f.libraries.len() == before { (404, None) } else { (204, None) }
                }
                ("GET", "/Items/Latest") if f.keys.iter().any(|(_, k)| *k == token(r)) => {
                    if r.query("userId").is_none() {
                        return (400, Some("Error processing request.".into()));
                    }
                    (200, Some(r#"[{"Name":"Moana","ProductionYear":2016,"Type":"Movie"},{"Name":"Clip","Type":"Movie"}]"#.into()))
                }
                ("POST", "/Library/Refresh") if f.keys.iter().any(|(_, k)| *k == token(r)) => {
                    f.scanning = true;
                    (204, None)
                }
                ("GET", "/ScheduledTasks") if f.keys.iter().any(|(_, k)| *k == token(r)) => {
                    let scan = if f.scanning {
                        json!({"Key": "RefreshLibrary", "State": "Running", "CurrentProgressPercentage": 48.0,
                               "LastExecutionResult": {"Status": "Completed"}})
                    } else {
                        json!({"Key": "RefreshLibrary", "State": "Idle", "LastExecutionResult": {"Status": "Failed"}})
                    };
                    (200, Some(json!([{"Key": "CleanLogFiles", "State": "Idle"}, scan]).to_string()))
                }
                ("POST", "/Users/New") if f.keys.iter().any(|(_, k)| *k == token(r)) => {
                    let name = body["Name"].as_str().unwrap().to_string();
                    if f.users.iter().any(|u| u.1.eq_ignore_ascii_case(&name)) {
                        return (400, Some("Error processing request.".into()));
                    }
                    let id = format!("u{}", f.users.len() + 1);
                    f.users.push((id.clone(), name.clone(), false, body["Password"].as_str().unwrap_or("").into()));
                    (200, Some(json!({"Id": id, "Name": name, "HasPassword": true, "Policy": {"IsAdministrator": false}}).to_string()))
                }
                ("GET", "/Users") if f.keys.iter().any(|(_, k)| *k == token(r)) => {
                    let items: Vec<_> = f
                        .users
                        .iter()
                        .map(|(id, n, admin, pw)| json!({"Id": id, "Name": n, "HasPassword": !pw.is_empty(), "Policy": {"IsAdministrator": admin}}))
                        .collect();
                    (200, Some(json!(items).to_string()))
                }
                ("POST", p) if p.starts_with("/Users/") && p.ends_with("/Password") && f.keys.iter().any(|(_, k)| *k == token(r)) => {
                    let id = p.trim_start_matches("/Users/").trim_end_matches("/Password").to_string();
                    let pw = body["NewPw"].as_str().unwrap_or("").to_string();
                    match f.users.iter_mut().find(|u| u.0 == id) {
                        Some(u) => {
                            u.3 = pw;
                            (204, None)
                        }
                        None => (404, None),
                    }
                }
                _ => (401, None),
            }
        });
        Client::new(&serve(handler), "LAPTOP")
    }

    fn simple(status: u16, body: &'static str) -> Client {
        Client::new(&serve(Arc::new(move |_r: &Request| (status, Some(body.to_string())))), "test-host")
    }

    #[test]
    fn healthy_means_the_body_says_healthy() {
        assert!(simple(200, "Healthy").healthy());
        assert!(!simple(503, "starting").healthy());
        // 12.4 answers 200 "Degraded" while it is still migrating.
        assert!(!simple(200, "Degraded").healthy());
    }

    #[test]
    fn requests_time_out() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        // Accepts and never answers.
        std::thread::spawn(move || {
            let _held: Vec<_> = listener.incoming().flatten().collect();
        });
        let c = Client::with_timeout(&base, "t", Duration::from_millis(100));
        let start = std::time::Instant::now();
        assert!(!c.healthy());
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn sessions_are_read_with_the_key_in_the_authorization_header() {
        let seen = Arc::new(Mutex::new((String::new(), String::new())));
        let s = seen.clone();
        let base = serve(Arc::new(move |r: &Request| {
            *s.lock().unwrap() = (r.header("Authorization").unwrap_or("").to_string(), r.target.clone());
            (200, Some(r#"[
              {"DeviceName":"Living room TV","Client":"Jellyfin Android TV",
               "NowPlayingItem":{"Name":"Film","RunTimeTicks":72000000000},
               "PlayState":{"PositionTicks":36000000000,"IsPaused":false}},
              {"DeviceName":"Phone","Client":"Jellyfin Web","PlayState":{"IsPaused":false}}]"#.into()))
        }));
        let got = Client::new(&base, "test-host").sessions("k3y").unwrap();
        let (auth, target) = seen.lock().unwrap().clone();
        assert_eq!(
            auth,
            format!(r#"MediaBrowser Client="Remora", Device="test-host", DeviceId="remora-test-host", Version="{}", Token="k3y""#, env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(target, "/Sessions?activeWithinSeconds=960");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].item, Some(Item { name: "Film".into(), runtime_ticks: 72000000000 }));
        assert_eq!(got[0].position_ticks, 36000000000);
        assert_eq!(got[0].device, "Living room TV");
        assert!(got[1].item.is_none());
    }

    #[test]
    fn a_rejected_key_is_its_own_error_and_other_failures_are_not() {
        assert_eq!(simple(401, "").sessions("revoked"), Err(Error::Unauthorized));
        assert!(matches!(simple(500, "").sessions("k"), Err(Error::Status { status: 500, .. })));
    }

    #[test]
    fn needs_setup_reads_the_wizard_flag() {
        let state = Arc::new(Mutex::new(Fake::default()));
        let c = fake(state.clone());
        assert_eq!(c.needs_setup(), Ok(true));
        state.lock().unwrap().complete = true;
        assert_eq!(c.needs_setup(), Ok(false));
    }

    #[test]
    fn setup_runs_the_wizard_with_remote_access_off() {
        let state = Arc::new(Mutex::new(Fake::default()));
        fake(state.clone()).setup("alex", "pw", r"D:\Media").unwrap();
        let f = state.lock().unwrap();
        assert!(f.complete);
        assert_eq!((f.user.as_str(), f.pw.as_str()), ("alex", "pw"));
        assert_eq!(f.library, r"Media=D:\Media");
        assert_eq!(f.remote, Some(false));
        let at = |c: &str| f.calls.iter().position(|x| x == c).unwrap();
        assert!(at("GET /Startup/User") < at("POST /Startup/User"));
        assert!(at("POST /Library/VirtualFolders") < at("POST /Startup/Complete"));
    }

    #[test]
    fn setup_without_a_media_folder_adds_no_library() {
        let state = Arc::new(Mutex::new(Fake::default()));
        fake(state.clone()).setup("alex", "pw", "  ").unwrap();
        let f = state.lock().unwrap();
        assert!(f.complete && f.library.is_empty());
    }

    #[test]
    fn setup_refuses_an_empty_name_or_password() {
        let state = Arc::new(Mutex::new(Fake::default()));
        let c = fake(state.clone());
        assert!(c.setup("", "pw", "").is_err());
        assert!(c.setup("alex", "", "").is_err());
        assert!(state.lock().unwrap().calls.is_empty());
    }

    #[test]
    fn make_key_takes_the_new_key_and_logs_out() {
        let state = Arc::new(Mutex::new(Fake { complete: true, user: "alex".into(), pw: "pw".into(), ..Default::default() }));
        // A key left by an earlier run under the same app name.
        state.lock().unwrap().keys.push(("remora-LAPTOP".into(), "stale".into()));
        let key = fake(state.clone()).make_key("alex", "pw").unwrap();
        assert!(key.starts_with("key-"), "{key}");
        let f = state.lock().unwrap();
        assert!(f.sessions.is_empty(), "the session token survived");
        assert_eq!(f.logged_out.len(), 1);
        assert_eq!(f.keys.last().unwrap().0, "remora-LAPTOP");
    }

    fn with_users() -> Arc<Mutex<Fake>> {
        let mut f = Fake { complete: true, ..Default::default() };
        f.keys.push(("remora-LAPTOP".into(), "k".into()));
        f.users = vec![("f1".into(), "kids".into(), false, String::new()), ("a1".into(), "alex".into(), true, "old".into())];
        Arc::new(Mutex::new(f))
    }

    #[test]
    fn recently_added_reads_names_and_years() {
        let got = fake(with_users()).recently_added("k", "a1").unwrap();
        assert_eq!(got, vec![Recent { name: "Moana".into(), year: Some(2016) }, Recent { name: "Clip".into(), year: None }]);
        assert_eq!(fake(with_users()).recently_added("revoked", "a1"), Err(Error::Unauthorized));
    }

    #[test]
    fn libraries_are_listed_and_a_scan_reports_progress() {
        let state = with_users();
        state.lock().unwrap().libraries = vec![("Films".into(), vec![r"D:\Films".into()])];
        let c = fake(state.clone());
        assert_eq!(c.libraries("k").unwrap(), vec![Library { name: "Films".into(), paths: vec![r"D:\Films".into()] }]);
        assert_eq!(c.scan_state("k").unwrap(), Scan { running: false, percent: None, last: Some("Failed".into()) });
        c.scan("k").unwrap();
        assert_eq!(c.scan_state("k").unwrap(), Scan { running: true, percent: Some(48.0), last: Some("Completed".into()) });
        assert_eq!(c.scan("revoked"), Err(Error::Unauthorized));
        assert_eq!(c.libraries("revoked"), Err(Error::Unauthorized));
    }

    #[test]
    fn libraries_and_folders_are_added_and_removed_with_encoded_names() {
        let state = with_users();
        let c = fake(state.clone());
        c.add_library("k", "Kids & Co", "movies", r"D:\Kids Films").unwrap();
        c.add_folder("k", "Kids & Co", r"E:\More").unwrap();
        assert_eq!(state.lock().unwrap().libraries, vec![("Kids & Co".to_string(), vec![r"D:\Kids Films".to_string(), r"E:\More".to_string()])]);
        assert!(state.lock().unwrap().calls.contains(&"kind=movies refresh=true".to_string()));
        assert!(matches!(c.add_folder("k", "Kids & Co", r"D:\missing"), Err(Error::Status { status: 404, .. })));
        c.remove_folder("k", "Kids & Co", r"E:\More").unwrap();
        assert_eq!(state.lock().unwrap().libraries[0].1, vec![r"D:\Kids Films".to_string()]);
        c.remove_library("k", "Kids & Co").unwrap();
        assert!(state.lock().unwrap().libraries.is_empty());
        assert_eq!(c.remove_library("revoked", "x"), Err(Error::Unauthorized));
    }

    #[test]
    fn users_are_listed_with_the_key_admins_first() {
        let state = with_users();
        let got = fake(state).users("k").unwrap();
        assert_eq!(
            got,
            vec![
                User { id: "a1".into(), name: "alex".into(), admin: true, has_password: true },
                User { id: "f1".into(), name: "kids".into(), admin: false, has_password: false },
            ]
        );
        assert_eq!(fake(with_users()).users("revoked"), Err(Error::Unauthorized));
    }

    #[test]
    fn add_user_posts_the_name_and_password_with_the_key() {
        let state = with_users();
        let u = fake(state.clone()).add_user("k", "grandma", "pw").unwrap();
        assert_eq!((u.name.as_str(), u.admin), ("grandma", false));
        assert!(state.lock().unwrap().users.iter().any(|x| x.1 == "grandma" && x.3 == "pw"));
        assert_eq!(fake(state).add_user("revoked", "x", ""), Err(Error::Unauthorized));
    }

    #[test]
    fn set_password_sends_only_the_new_one_with_the_key() {
        let state = with_users();
        fake(state.clone()).set_password("k", "a1", "new").unwrap();
        assert_eq!(state.lock().unwrap().users[1].3, "new");
        fake(state.clone()).set_password("k", "f1", "").unwrap();
        assert_eq!(state.lock().unwrap().users[0].3, "");
        assert_eq!(fake(state.clone()).set_password("revoked", "a1", "x"), Err(Error::Unauthorized));
        assert!(matches!(fake(state).set_password("k", "gone", "x"), Err(Error::Status { status: 404, .. })));
    }

    #[test]
    fn make_key_with_a_wrong_password_says_so_and_makes_nothing() {
        let state = Arc::new(Mutex::new(Fake { complete: true, user: "alex".into(), pw: "pw".into(), ..Default::default() }));
        assert_eq!(fake(state.clone()).make_key("alex", "nope"), Err(Error::BadPassword));
        assert!(state.lock().unwrap().keys.is_empty());
    }
}
