//! What GitHub knows about commit authors (login, profile name, avatar), looked up in a
//! background thread and cached on disk, so the second run is instant and works offline.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bevy::log::warn;
use serde::{Deserialize, Serialize};
use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig};

use crate::identity::noreply_account;

/// How long a cached answer is trusted, and a cached "GitHub doesn't know".
const CACHE_DAYS: u64 = 30;
const CACHE_MISS_DAYS: u64 = 7;
/// The cache is saved every this many network answers, in case the app is closed early.
const SAVE_EVERY: usize = 50;
/// Requests in flight at once: much faster than one by one, and polite enough for GitHub.
const WORKERS: usize = 8;
/// With this many emails unknown, listing the repository's commits beats asking per email…
const BULK_MIN_EMAILS: usize = 20;
/// …a page of this many commits at a time, for at most this many pages (100k commits).
const COMMIT_PAGE: usize = 100;
const MAX_COMMIT_PAGES: usize = 1000;

pub struct Identity {
    pub email: String,
    pub login: Option<String>,
    /// The name on their GitHub profile.
    pub profile: Option<String>,
    /// Image bytes and their MIME type.
    pub avatar: Option<(Vec<u8>, String)>,
}

/// Resolves each `(email, sha of one of their commits, its repository)` in a background
/// thread, sending identities as they arrive so the visualization never waits on the network.
/// Authors come first in the order given (put those seen first on screen first). The channel
/// closes once every author is done.
pub fn resolve(authors: Vec<(String, String, PathBuf)>) -> Receiver<Identity> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let github = GitHub::new();
        let accounts = github.accounts(&authors);
        // Profiles and avatars, a few at a time, sent as soon as each is known.
        let next = AtomicUsize::new(0);
        thread::scope(|scope| {
            for _ in 0..WORKERS {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((email, _, _)) = authors.get(i) else {
                            break;
                        };
                        let identity = github.identity(email.clone(), accounts[i].clone());
                        if tx.send(identity).is_err() {
                            // The app is gone: stop everyone.
                            next.store(authors.len(), Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        github.save();
    });
    rx
}

/// An account on GitHub: its avatar URL and login.
type Account = Option<(String, String)>;

struct GitHub {
    agent: Agent,
    cache: Mutex<Cache>,
    /// `owner/name` on GitHub of each repository, looked up once.
    repos: Mutex<HashMap<PathBuf, Option<String>>>,
    token: OnceLock<Option<String>>,
    /// Set once GitHub refuses requests (rate limit): no more API calls this run.
    limited: AtomicBool,
    unsaved: AtomicUsize,
}

/// A failed request. Only `NotFound` (a renamed account, a missing avatar) is cached; anything
/// else is asked again next time.
enum Failed {
    NotFound,
    Other,
}

impl GitHub {
    fn new() -> Self {
        // Trust the OS certificate store, so corporate TLS proxies work like in the browser.
        let tls = TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let agent: Agent = Agent::config_builder().tls_config(tls).build().into();
        Self {
            agent,
            cache: Mutex::new(Cache::load()),
            repos: Mutex::new(HashMap::new()),
            token: OnceLock::new(),
            limited: AtomicBool::new(false),
            unsaved: AtomicUsize::new(0),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Notes a new answer in the cache, saving it now and then in case the app is closed early.
    fn remember(&self, store: impl FnOnce(&mut Cache)) {
        let mut cache = self.cache();
        store(&mut cache);
        if self.unsaved.fetch_add(1, Ordering::Relaxed) + 1 >= SAVE_EVERY {
            self.unsaved.store(0, Ordering::Relaxed);
            cache.save();
        }
    }

    fn save(&self) {
        self.cache().save();
    }

    /// The account of each author: from a noreply email, the cache, or GitHub. With many
    /// unknown emails, listing a repository's commits (100 per request) finds them far faster
    /// than one request per email; the rest are then asked one by one.
    fn accounts(&self, authors: &[(String, String, PathBuf)]) -> Vec<Account> {
        let mut accounts: Vec<Option<Account>> = authors
            .iter()
            .map(|(email, _, _)| {
                noreply_avatar(email).map(Some).or_else(|| {
                    self.cache()
                        .emails
                        .get(email)
                        .filter(|c| c.fresh())
                        .map(|c| c.value.clone())
                })
            })
            .collect();
        let unknown: Vec<usize> = (0..authors.len())
            .filter(|&i| accounts[i].is_none())
            .collect();
        if unknown.len() >= BULK_MIN_EMAILS {
            let mut by_repo: HashMap<String, HashSet<&str>> = HashMap::new();
            for &i in &unknown {
                if let Some(github) = self.repo(&authors[i].2) {
                    by_repo
                        .entry(github)
                        .or_default()
                        .insert(authors[i].0.as_str());
                }
            }
            for (repo, emails) in by_repo {
                let found = self.list_commits(&repo, &emails);
                for &i in &unknown {
                    if let Some(account) = found.get(authors[i].0.as_str()) {
                        self.remember(|cache| {
                            cache
                                .emails
                                .insert(authors[i].0.clone(), Cached::now(account.clone()));
                        });
                        accounts[i] = Some(account.clone());
                    }
                }
            }
        }
        // The rest, one request each (a few at a time).
        let rest: Vec<usize> = (0..authors.len())
            .filter(|&i| accounts[i].is_none())
            .collect();
        let next = AtomicUsize::new(0);
        let answers: Mutex<Vec<(usize, Account)>> = Mutex::new(Vec::new());
        thread::scope(|scope| {
            for _ in 0..WORKERS {
                scope.spawn(|| {
                    while let Some(&i) = rest.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let (email, sha, repo) = &authors[i];
                        let Some(found) = self
                            .repo(repo)
                            .and_then(|github| self.lookup(&github, sha).ok())
                        else {
                            continue;
                        };
                        self.remember(|cache| {
                            cache
                                .emails
                                .insert(email.clone(), Cached::now(found.clone()));
                        });
                        answers
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .push((i, found));
                    }
                });
            }
        });
        for (i, account) in answers.into_inner().unwrap_or_else(|p| p.into_inner()) {
            accounts[i] = Some(account);
        }
        accounts.into_iter().map(Option::flatten).collect()
    }

    /// Pages through a repository's commits (a few pages at a time) until every email in
    /// `emails` has been seen or the history ends: each email's account, or `None` when its
    /// commits aren't tied to one.
    fn list_commits(&self, repo: &str, emails: &HashSet<&str>) -> HashMap<String, Account> {
        let found: Mutex<HashMap<String, Account>> = Mutex::new(HashMap::new());
        let (next, done) = (AtomicUsize::new(1), AtomicBool::new(false));
        thread::scope(|scope| {
            for _ in 0..WORKERS {
                scope.spawn(|| {
                    while !done.load(Ordering::Relaxed) {
                        let page = next.fetch_add(1, Ordering::Relaxed);
                        if page > MAX_COMMIT_PAGES {
                            break;
                        }
                        let url = format!("https://api.github.com/repos/{repo}/commits?per_page={COMMIT_PAGE}&page={page}");
                        let Ok(serde_json::Value::Array(commits)) = self.get(&url) else {
                            done.store(true, Ordering::Relaxed);
                            break;
                        };
                        let mut found = found.lock().unwrap_or_else(|p| p.into_inner());
                        note_accounts(&commits, emails, &mut found);
                        if commits.len() < COMMIT_PAGE || found.len() == emails.len() {
                            done.store(true, Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        found.into_inner().unwrap_or_else(|p| p.into_inner())
    }

    fn identity(&self, email: String, account: Account) -> Identity {
        let profile = account.as_ref().and_then(|(_, login)| self.profile(login));
        let avatar = account.as_ref().and_then(|(url, _)| self.avatar(url));
        Identity {
            email,
            login: account.map(|(_, login)| login),
            profile,
            avatar,
        }
    }

    /// `owner/name` of the repository's `origin` remote, if it is hosted on GitHub.
    fn repo(&self, repo: &Path) -> Option<String> {
        let mut repos = self.repos.lock().unwrap_or_else(|p| p.into_inner());
        repos
            .entry(repo.to_path_buf())
            .or_insert_with_key(|repo| github_repo(repo))
            .clone()
    }

    fn get(&self, url: &str) -> Result<serde_json::Value, Failed> {
        if self.limited.load(Ordering::Relaxed) {
            return Err(Failed::Other);
        }
        let token = self.token.get_or_init(token);
        let mut request = self
            .agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "eridu");
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let body = request
            .call()
            .and_then(|mut response| response.body_mut().read_to_string());
        match body {
            Ok(body) => serde_json::from_str(&body).map_err(|_| Failed::Other),
            Err(ureq::Error::StatusCode(404)) => Err(Failed::NotFound),
            Err(ureq::Error::StatusCode(code @ (403 | 429))) => {
                if !self.limited.swap(true, Ordering::Relaxed) {
                    warn!(
                        "GitHub refused {url} ({code}): rate limited, no more lookups this run (set GITHUB_TOKEN)"
                    );
                }
                Err(Failed::Other)
            }
            Err(e) => {
                warn!("GitHub {url}: {e}");
                Err(Failed::Other)
            }
        }
    }

    /// Asks GitHub which account authored `sha`: its avatar URL and login, or `None` when the
    /// email isn't tied to an account. Listing from that sha (rather than fetching the commit
    /// itself) keeps the response small: it skips the diff.
    fn lookup(&self, repo: &str, sha: &str) -> Result<Account, Failed> {
        let commits = self.get(&format!(
            "https://api.github.com/repos/{repo}/commits?sha={sha}&per_page=1"
        ))?;
        Ok(commit_account(&commits[0]))
    }

    /// The name on a GitHub profile, which gives a first name when commits don't.
    fn profile(&self, login: &str) -> Option<String> {
        if login.ends_with("[bot]") {
            return None;
        }
        if let Some(cached) = self.cache().profiles.get(login).filter(|c| c.fresh()) {
            return cached.value.clone();
        }
        let name = match self.get(&format!("https://api.github.com/users/{login}")) {
            Ok(user) => profile_name(&user),
            Err(Failed::NotFound) => None,
            Err(Failed::Other) => return None,
        };
        self.remember(|cache| {
            cache
                .profiles
                .insert(login.to_string(), Cached::now(name.clone()));
        });
        name
    }

    fn avatar(&self, url: &str) -> Option<(Vec<u8>, String)> {
        let file = self.cache().avatar_file(url);
        // An empty file remembers that there is no avatar.
        if let Some(bytes) = file.as_deref().and_then(read_fresh) {
            let mime = mime(&bytes).to_string();
            return (!bytes.is_empty()).then_some((bytes, mime));
        }
        let fetch = || -> Result<(Vec<u8>, String), ureq::Error> {
            let mut response = self.agent.get(url).header("User-Agent", "eridu").call()?;
            let mime = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("image/png");
            let mime = mime.to_string();
            Ok((response.body_mut().read_to_vec()?, mime))
        };
        let (bytes, mime) = match fetch() {
            Ok(fetched) => fetched,
            Err(ureq::Error::StatusCode(404)) => (Vec::new(), String::new()),
            Err(e) => {
                warn!("avatar {url}: {e}");
                return None;
            }
        };
        if let Some(file) = file {
            let _ = std::fs::create_dir_all(file.parent().expect("in the cache folder"));
            let _ = std::fs::write(&file, &bytes);
        }
        (!bytes.is_empty()).then_some((bytes, mime))
    }
}

/// Answers from GitHub kept between runs, in the user's cache folder.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Cache {
    /// By email: the account (avatar URL, login), or `None` when GitHub knows no account.
    emails: HashMap<String, Cached<Option<(String, String)>>>,
    /// By login: the profile name.
    profiles: HashMap<String, Cached<Option<String>>>,
    #[serde(skip)]
    dir: Option<PathBuf>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Cached<T> {
    value: T,
    /// Unix time it was asked.
    at: u64,
}

impl<T> Cached<T> {
    fn now(value: T) -> Self {
        Self {
            value,
            at: unix_now(),
        }
    }
}

impl<T> Cached<Option<T>> {
    /// "GitHub doesn't know" is asked again sooner than an answer.
    fn fresh(&self) -> bool {
        let days = if self.value.is_none() {
            CACHE_MISS_DAYS
        } else {
            CACHE_DAYS
        };
        unix_now().saturating_sub(self.at) < days * 86400
    }
}

impl Cache {
    fn load() -> Self {
        cache_dir().map_or_else(Self::default, Self::load_from)
    }

    /// The cache saved in `dir`, empty if there is none (or it can't be read).
    fn load_from(dir: PathBuf) -> Self {
        let text = std::fs::read_to_string(dir.join("identities.json")).unwrap_or_default();
        let mut cache: Cache = serde_json::from_str(&text).unwrap_or_default();
        cache.dir = Some(dir);
        cache
    }

    /// Writes the cache through a temporary file, so a crash never leaves it half written.
    fn save(&self) {
        let Some(dir) = &self.dir else {
            return;
        };
        let Ok(text) = serde_json::to_string(self) else {
            return;
        };
        let (file, temporary) = (dir.join("identities.json"), dir.join("identities.json.tmp"));
        let saved = std::fs::create_dir_all(dir)
            .and_then(|_| std::fs::write(&temporary, text))
            .and_then(|_| std::fs::rename(&temporary, &file));
        if let Err(e) = saved {
            warn!("cannot save the identity cache in {}: {e}", dir.display());
        }
    }

    fn avatar_file(&self, url: &str) -> Option<PathBuf> {
        Some(
            self.dir
                .as_ref()?
                .join("avatars")
                .join(format!("{:016x}", fnv1a(url))),
        )
    }
}

/// `%LOCALAPPDATA%\eridu` on Windows, `$XDG_CACHE_HOME/eridu` or `~/.cache/eridu` elsewhere.
fn cache_dir() -> Option<PathBuf> {
    let var = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = var("LOCALAPPDATA")
        .or_else(|| var("XDG_CACHE_HOME"))
        .or_else(|| var("HOME").map(|home| home.join(".cache")))?;
    Some(base.join("eridu"))
}

/// A file's bytes, unless it is missing or older than the cache allows.
fn read_fresh(file: &Path) -> Option<Vec<u8>> {
    let age = std::fs::metadata(file)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .unwrap_or_default();
    if age < Duration::from_secs(CACHE_DAYS * 86400) {
        std::fs::read(file).ok()
    } else {
        None
    }
}

/// The MIME type of an image, from its first bytes.
fn mime(bytes: &[u8]) -> &'static str {
    match bytes {
        [0xFF, 0xD8, ..] => "image/jpeg",
        [b'G', b'I', b'F', ..] => "image/gif",
        _ => "image/png",
    }
}

/// A stable 64-bit hash (std's `DefaultHasher` may change between Rust versions).
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `owner/name` of the repository's `origin` remote, if it is hosted on GitHub.
fn github_repo(repo: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    github_slug(&String::from_utf8(out.stdout).ok()?)
}

/// `owner/name` of a GitHub remote URL: git@github.com:owner/name.git, https://github.com/owner/name.
fn github_slug(url: &str) -> Option<String> {
    let (host, path) = url.trim().split_once("github.com")?;
    // The host itself, not one that merely ends with it (notgithub.com, github.com.evil.org).
    let whole_host = host.is_empty() || host.ends_with(['@', '/', '.']);
    if !whole_host || !path.starts_with([':', '/']) {
        return None;
    }
    let path = path
        .trim_start_matches([':', '/'])
        .trim_end_matches('/')
        .trim_end_matches(".git");
    path.contains('/').then(|| path.to_string())
}

/// The account behind a GitHub noreply email, known without asking: the numeric id gives the
/// avatar (`github.com/<login>.png` 404s for bots and enterprise accounts); older addresses
/// only have the login.
fn noreply_avatar(email: &str) -> Account {
    Some(match noreply_account(email)? {
        (Some(id), login) => (
            format!("https://avatars.githubusercontent.com/u/{id}?s=64"),
            login,
        ),
        (None, login) => (format!("https://github.com/{login}.png?size=64"), login),
    })
}

/// The account of a commit in the API's answer: its avatar URL and login, or `None` when the
/// email isn't tied to one. The API's avatar URL also works for bots and enterprise accounts,
/// unlike github.com/<login>.png.
fn commit_account(commit: &serde_json::Value) -> Account {
    let account = &commit["author"];
    account["avatar_url"]
        .as_str()
        .zip(account["login"].as_str())
        .map(|(avatar, login)| (format!("{avatar}&s=64"), login.to_string()))
}

/// Notes the account of each commit of a page whose author email is in `emails`.
fn note_accounts(
    commits: &[serde_json::Value],
    emails: &HashSet<&str>,
    found: &mut HashMap<String, Account>,
) {
    for commit in commits {
        let Some(email) = commit["commit"]["author"]["email"]
            .as_str()
            .filter(|e| emails.contains(e))
        else {
            continue;
        };
        // An account seen on any of their commits wins over one without.
        let entry = found.entry(email.to_string()).or_insert(None);
        if entry.is_none() {
            *entry = commit_account(commit);
        }
    }
}

/// The name on a user's profile, from the API's answer (often missing or blank).
fn profile_name(user: &serde_json::Value) -> Option<String> {
    user["name"]
        .as_str()
        .map(str::to_string)
        .filter(|n| !n.trim().is_empty())
}

/// A GitHub token from the environment or the `gh` CLI, needed for private repos and higher rate limits.
fn token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok()
        .or_else(|| {
            let out = Command::new("gh").args(["auth", "token"]).output().ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        })
        .filter(|token| !token.is_empty())
}

/// The images of an `--avatar-dir`, by lowercased file name without extension.
pub fn avatar_files(dir: &Path) -> HashMap<String, PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        warn!("avatar-dir: cannot read {}", dir.display());
        return HashMap::new();
    };
    entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let extension = path.extension()?.to_str()?.to_lowercase();
            matches!(extension.as_str(), "png" | "jpg" | "jpeg").then_some(())?;
            Some((path.file_stem()?.to_str()?.to_lowercase(), path))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A fresh folder under the system's temp dir, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "eridu-test-{name}-{}-{}",
                std::process::id(),
                unix_now()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn account(avatar: &str, login: &str) -> Account {
        Some((avatar.to_string(), login.to_string()))
    }

    fn days_ago(days: u64) -> u64 {
        unix_now() - days * 86400
    }

    #[test]
    fn remote_urls_give_owner_and_name() {
        let expected = Some("ada/engine".to_string());
        assert_eq!(github_slug("git@github.com:ada/engine.git\n"), expected);
        assert_eq!(github_slug("git@github.com:ada/engine"), expected);
        assert_eq!(github_slug("https://github.com/ada/engine.git"), expected);
        assert_eq!(github_slug("https://github.com/ada/engine/"), expected);
        assert_eq!(github_slug("https://token@github.com/ada/engine"), expected);
        assert_eq!(github_slug("ssh://git@github.com/ada/engine.git"), expected);
        // A dot in the name is kept, only the `.git` suffix goes.
        assert_eq!(
            github_slug("https://github.com/ada/ada.github.io.git"),
            Some("ada/ada.github.io".to_string())
        );
    }

    #[test]
    fn other_hosts_and_bare_owners_give_nothing() {
        assert_eq!(github_slug("git@gitlab.com:ada/engine.git"), None);
        assert_eq!(github_slug("https://bitbucket.org/ada/engine"), None);
        assert_eq!(github_slug("https://github.com/ada"), None);
        assert_eq!(github_slug(""), None);
    }

    #[test]
    fn look_alike_hosts_are_not_github() {
        assert_eq!(github_slug("https://notgithub.com/ada/engine"), None);
    }

    #[test]
    fn noreply_emails_give_an_account_without_asking() {
        assert_eq!(
            noreply_avatar("1815+ada@users.noreply.github.com"),
            account("https://avatars.githubusercontent.com/u/1815?s=64", "ada")
        );
        // Older noreply addresses have no id: only the login's picture.
        assert_eq!(
            noreply_avatar("grace@users.noreply.github.com"),
            account("https://github.com/grace.png?size=64", "grace")
        );
        assert_eq!(noreply_avatar("ada@acme.com"), None);
    }

    #[test]
    fn commit_answers_give_avatar_and_login() {
        let commit = json!({
            "commit": {"author": {"email": "ada@acme.com"}},
            "author": {
                "login": "ada",
                "avatar_url": "https://avatars.githubusercontent.com/u/1815?v=4"
            }
        });
        assert_eq!(
            commit_account(&commit),
            account(
                "https://avatars.githubusercontent.com/u/1815?v=4&s=64",
                "ada"
            )
        );
        // An email tied to no account: `author` is null.
        let unknown = json!({"commit": {"author": {"email": "x@acme.com"}}, "author": null});
        assert_eq!(commit_account(&unknown), None);
        // `lookup` reads the first commit of a list; an empty list means no account.
        assert_eq!(commit_account(&json!([])[0]), None);
    }

    #[test]
    fn commit_pages_find_the_asked_emails() {
        let commit = |email: &str, login: Option<&str>| match login {
            Some(login) => json!({
                "commit": {"author": {"email": email}},
                "author": {"login": login, "avatar_url": format!("https://a/{login}?v=4")}
            }),
            None => json!({"commit": {"author": {"email": email}}, "author": null}),
        };
        let emails = HashSet::from(["ada@acme.com", "grace@acme.com", "alan@acme.com"]);
        let mut found = HashMap::new();
        note_accounts(
            &[
                commit("ada@acme.com", None),
                commit("ada@acme.com", Some("ada")),
                commit("grace@acme.com", Some("grace")),
                commit("grace@acme.com", None),
                commit("linus@acme.com", Some("linus")),
            ],
            &emails,
            &mut found,
        );
        // An account seen on any commit wins, whatever the order.
        assert_eq!(
            found["ada@acme.com"],
            account("https://a/ada?v=4&s=64", "ada")
        );
        assert_eq!(
            found["grace@acme.com"],
            account("https://a/grace?v=4&s=64", "grace")
        );
        // Not asked for, or not seen yet.
        assert!(!found.contains_key("linus@acme.com"));
        assert!(!found.contains_key("alan@acme.com"));
        // Seen only without an account: known to have none.
        note_accounts(&[commit("alan@acme.com", None)], &emails, &mut found);
        assert_eq!(found["alan@acme.com"], None);
    }

    #[test]
    fn profile_names_skip_blank_ones() {
        assert_eq!(
            profile_name(&json!({"login": "ada", "name": "Ada Lovelace"})),
            Some("Ada Lovelace".to_string())
        );
        assert_eq!(profile_name(&json!({"login": "ada", "name": null})), None);
        assert_eq!(profile_name(&json!({"login": "ada", "name": "  "})), None);
        assert_eq!(profile_name(&json!({"login": "ada"})), None);
    }

    #[test]
    fn answers_are_trusted_longer_than_misses() {
        let answer = |days| Cached {
            value: Some("Ada"),
            at: days_ago(days),
        };
        let miss = |days| Cached::<Option<&str>> {
            value: None,
            at: days_ago(days),
        };
        assert!(Cached::now(Some("Ada")).fresh());
        assert!(answer(29).fresh());
        assert!(!answer(31).fresh());
        assert!(miss(6).fresh());
        assert!(!miss(8).fresh());
        // A clock set back doesn't expire anything.
        let future = Cached {
            value: Some("Ada"),
            at: unix_now() + 86400,
        };
        assert!(future.fresh());
    }

    #[test]
    fn the_cache_survives_a_save_and_a_load() {
        let temp = TempDir::new("cache");
        let dir = temp.0.join("eridu");
        let mut cache = Cache::load_from(dir.clone());
        assert!(cache.emails.is_empty() && cache.profiles.is_empty());
        cache.emails.insert(
            "ada@acme.com".to_string(),
            Cached::now(account("https://a/ada", "ada")),
        );
        cache
            .emails
            .insert("x@acme.com".to_string(), Cached::now(None));
        cache.profiles.insert(
            "ada".to_string(),
            Cached::now(Some("Ada Lovelace".to_string())),
        );
        // Saving creates the folder, and leaves no temporary file behind.
        cache.save();
        assert!(dir.join("identities.json").is_file());
        assert!(!dir.join("identities.json.tmp").exists());

        let loaded = Cache::load_from(dir.clone());
        assert_eq!(
            loaded.emails["ada@acme.com"].value,
            account("https://a/ada", "ada")
        );
        assert_eq!(loaded.emails["x@acme.com"].value, None);
        assert_eq!(
            loaded.profiles["ada"].value.as_deref(),
            Some("Ada Lovelace")
        );
        assert_eq!(loaded.dir.as_deref(), Some(dir.as_path()));
    }

    #[test]
    fn a_broken_or_partial_cache_loads() {
        let temp = TempDir::new("broken");
        std::fs::write(temp.0.join("identities.json"), "{ not json").unwrap();
        assert!(Cache::load_from(temp.0.clone()).emails.is_empty());
        // Missing fields default, so an older cache file still loads.
        let at = unix_now();
        std::fs::write(
            temp.0.join("identities.json"),
            format!(r#"{{"profiles": {{"ada": {{"value": "Ada", "at": {at}}}}}}}"#),
        )
        .unwrap();
        let cache = Cache::load_from(temp.0.clone());
        assert!(cache.emails.is_empty());
        assert_eq!(cache.profiles["ada"].value.as_deref(), Some("Ada"));
    }

    #[test]
    fn a_cache_without_a_folder_saves_nothing() {
        let cache = Cache::default();
        assert_eq!(cache.avatar_file("https://a/ada"), None);
        cache.save();
    }

    #[test]
    fn avatar_files_are_stable_per_url() {
        let cache = Cache {
            dir: Some(PathBuf::from("cache")),
            ..Cache::default()
        };
        let file = cache.avatar_file("https://a/ada").unwrap();
        assert_eq!(file, cache.avatar_file("https://a/ada").unwrap());
        assert_ne!(file, cache.avatar_file("https://a/grace").unwrap());
        assert!(file.starts_with(Path::new("cache").join("avatars")));
        // The hash must never change between builds, or every avatar is fetched again.
        assert_eq!(fnv1a(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(
            file.file_name().unwrap().to_str().unwrap(),
            format!("{:016x}", fnv1a("https://a/ada"))
        );
    }

    #[test]
    fn old_avatar_files_are_fetched_again() {
        let temp = TempDir::new("avatars");
        let file = temp.0.join("avatar");
        assert_eq!(read_fresh(&file), None);
        std::fs::write(&file, b"\x89PNG").unwrap();
        assert_eq!(read_fresh(&file), Some(b"\x89PNG".to_vec()));
        // An empty file is a remembered "no avatar", still fresh.
        std::fs::write(&file, b"").unwrap();
        assert_eq!(read_fresh(&file), Some(Vec::new()));
        let old = SystemTime::now() - Duration::from_secs((CACHE_DAYS + 1) * 86400);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(read_fresh(&file), None);
    }

    #[test]
    fn image_types_come_from_their_first_bytes() {
        assert_eq!(mime(&[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
        assert_eq!(mime(b"GIF89a"), "image/gif");
        assert_eq!(mime(b"\x89PNG\r\n"), "image/png");
        // Unknown or empty: PNG, the usual GitHub avatar.
        assert_eq!(mime(b""), "image/png");
    }

    #[test]
    fn avatar_dirs_list_images_by_lowercase_name() {
        let temp = TempDir::new("avatar-dir");
        for name in ["Ada.PNG", "grace.jpg", "alan.jpeg", "notes.txt", "linus"] {
            std::fs::write(temp.0.join(name), b"").unwrap();
        }
        let files = avatar_files(&temp.0);
        let mut names: Vec<&str> = files.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["ada", "alan", "grace"]);
        assert_eq!(files["ada"], temp.0.join("Ada.PNG"));
        assert!(avatar_files(&temp.0.join("missing")).is_empty());
    }
}
