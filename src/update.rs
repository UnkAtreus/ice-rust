//! Automatic updates (v1.3), the pure half: version numbers, the GitHub
//! release feed, when an automatic check is due, Ed25519 signatures over
//! the update zip, install paths and checks, and the relaunch helper's
//! arguments. The network, file system and AppKit parts live in
//! `platform::installer` and `platform::updater`.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde_json::Value;

use crate::base64;

/// The latest published release. Drafts and pre-releases are not listed
/// by this endpoint.
pub const FEED_URL: &str = "https://api.github.com/repos/UnkAtreus/ice-rust/releases/latest";
/// Where "Open Release Page" goes when the feed names no usable page.
pub const RELEASES_URL: &str = "https://github.com/UnkAtreus/ice-rust/releases";
/// Release pages the prompt may open.
const RELEASE_PAGE_PREFIX: &str = "https://github.com/UnkAtreus/ice-rust/";
/// Overrides the feed URL. Release builds accept `https://` only.
pub const FEED_ENV: &str = "ICE_RUST_UPDATE_FEED";

/// Bundle identifier an update must carry (`scripts/bundle.sh`).
pub const BUNDLE_ID: &str = "local.icerust.IceRust";
/// The single bundle an update zip must hold.
pub const BUNDLE_NAME: &str = "IceRust.app";
/// The version of this build.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// No automatic check in the first minute after launch.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
/// Time between successful automatic checks.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Minimum time between automatic attempts in one process, also failed ones.
pub const ATTEMPT_SPACING: Duration = Duration::from_secs(60 * 60);
/// How often the app asks `check_due`. Run loop timers stop while the Mac
/// sleeps, so the 24 h interval is measured on the wall clock instead.
pub const DUE_POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Longest release notes text the prompt shows.
pub const NOTES_MAX_CHARS: usize = 1500;
/// Largest update zip accepted, in bytes (curl's `--max-filesize 50M`).
pub const MAX_ZIP_BYTES: u64 = 50 * 1024 * 1024;
/// Largest signature file accepted, in bytes.
pub const MAX_SIG_BYTES: u64 = 4 * 1024;

/// Marks a build without a release key. Release tooling refuses it, and so
/// does every verification.
pub const PLACEHOLDER_PUBLIC_KEY: [u8; 32] = [0; 32];
/// The Ed25519 public key that signs update zips. Release maintainers put
/// the key that `ice-rust update-keygen` prints here; the private half
/// stays on the release machine.
pub const UPDATE_PUBLIC_KEY: [u8; 32] = [
    10, 171, 41, 217, 34, 14, 94, 225, 52, 102, 207, 45, 77, 171, 12, 173, 226, 188, 68, 154, 157,
    137, 25, 111, 189, 186, 107, 193, 184, 164, 15, 135,
];

/// Defaults keys (v1.3).
pub const KEY_LAST_UPDATE_CHECK: &str = "LastUpdateCheck";
pub const KEY_SKIPPED_VERSION: &str = "SkippedUpdateVersion";
pub const KEY_LAST_LAUNCHED_VERSION: &str = "LastLaunchedVersion";

pub const MSG_UP_TO_DATE: &str = "ice-rust is up to date";
/// Shown in the update prompt. Releases from 0.1.5 on are signed with the
/// ice-rust certificate, so TCC grants carry over to the next one.
pub const PERMISSIONS_NOTE: &str = "Screen Recording and Accessibility carry over to the new \
     version. If one stops working, remove IceRust from that list in Privacy & Security with \
     the minus (-) button, then open IceRust and grant it again. Turning the switch off and on \
     does not help.";

/// Shown once after a version change with a missing permission. TCC keeps
/// the code requirement of the build that was granted; turning the switch
/// off and on keeps that requirement, removing the entry drops it.
pub const REGRANT_NOTE: &str = "macOS does not apply the earlier grants to this version, even \
     where System Settings still shows IceRust switched on. In Privacy & Security > Screen \
     Recording and in Privacy & Security > Accessibility, select IceRust and remove it with the \
     minus (-) button. Turning the switch off and on does not help. Then quit and open IceRust \
     and grant both when it asks.";

/// Whether `key` is the placeholder, which no update verifies against.
pub fn is_placeholder_key(key: &[u8; 32]) -> bool {
    *key == PLACEHOLDER_PUBLIC_KEY
}

/// `MAJOR.MINOR.PATCH`. Field order makes the derived ordering semantic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

fn version_part(text: &str) -> Option<u64> {
    let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    let leading_zero = text.len() > 1 && text.starts_with('0');
    if digits && !leading_zero {
        text.parse().ok()
    } else {
        None
    }
}

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Three decimal parts without leading zeros, as semantic versioning
    /// writes them. Pre-release and build suffixes are refused.
    pub fn parse(text: &str) -> Option<Version> {
        let parts: Vec<&str> = text.split('.').collect();
        let [major, minor, patch] = parts.as_slice() else {
            return None;
        };
        Some(Version::new(
            version_part(major)?,
            version_part(minor)?,
            version_part(patch)?,
        ))
    }

    /// A release tag, `vMAJOR.MINOR.PATCH`.
    pub fn from_tag(tag: &str) -> Option<Version> {
        tag.strip_prefix('v').and_then(Version::parse)
    }

    /// The running build's version. `CARGO_PKG_VERSION` always parses (a
    /// test checks it); if it did not, the highest version stands in so
    /// nothing is ever offered.
    pub fn current() -> Version {
        Version::parse(CURRENT_VERSION).unwrap_or(Version::new(u64::MAX, u64::MAX, u64::MAX))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// `IceRust-<version>-arm64.zip`.
pub fn zip_name(version: Version) -> String {
    format!("IceRust-{version}-arm64.zip")
}

/// `IceRust-<version>-arm64.zip.sig`.
pub fn sig_name(version: Version) -> String {
    format!("{}.sig", zip_name(version))
}

/// The version in an update zip's file name.
pub fn version_from_zip_name(name: &str) -> Option<Version> {
    name.strip_prefix("IceRust-")?
        .strip_suffix("-arm64.zip")
        .and_then(Version::parse)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// The fields of a GitHub release that the updater reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub draft: bool,
    pub prerelease: bool,
    pub notes: String,
    pub page_url: String,
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedError {
    Json(String),
    Missing(&'static str),
}

impl fmt::Display for FeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FeedError::Json(err) => write!(f, "the release feed is not valid JSON ({err})"),
            FeedError::Missing(field) => write!(f, "the release feed has no {field}"),
        }
    }
}

impl std::error::Error for FeedError {}

/// Reads a `releases/latest` response.
pub fn parse_release(json: &str) -> Result<Release, FeedError> {
    let value: Value = serde_json::from_str(json).map_err(|e| FeedError::Json(e.to_string()))?;
    let text = |key: &str| value.get(key).and_then(Value::as_str);
    let flag = |key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);
    let tag = text("tag_name").ok_or(FeedError::Missing("tag_name"))?;
    let assets = value
        .get("assets")
        .and_then(Value::as_array)
        .ok_or(FeedError::Missing("assets"))?
        .iter()
        .filter_map(|asset| {
            Some(Asset {
                name: asset.get("name")?.as_str()?.to_string(),
                url: asset.get("browser_download_url")?.as_str()?.to_string(),
                size: asset.get("size").and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .collect();
    Ok(Release {
        tag: tag.to_string(),
        draft: flag("draft"),
        prerelease: flag("prerelease"),
        notes: text("body").unwrap_or_default().to_string(),
        page_url: text("html_url").unwrap_or_default().to_string(),
        assets,
    })
}

/// An update the prompt can offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub version: Version,
    /// Plain, trimmed text (`plain_notes`, then `trim_notes`).
    pub notes: String,
    pub page_url: String,
    pub zip_url: String,
    pub sig_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Nothing newer. `latest` is the newest published version when its
    /// tag parses.
    UpToDate {
        latest: Option<Version>,
    },
    Available(Update),
    /// A newer release that cannot be installed in place.
    NotInstallable {
        version: Version,
        reason: String,
    },
}

fn is_https(url: &str) -> bool {
    url.starts_with("https://")
}

/// The release page to open, limited to this repository.
fn release_page(url: &str) -> String {
    if url.starts_with(RELEASE_PAGE_PREFIX) {
        url.to_string()
    } else {
        RELEASES_URL.to_string()
    }
}

/// Decides what a release means for the running `current` version. Only
/// a higher version with both signed assets is offered.
pub fn evaluate(current: Version, release: &Release) -> CheckOutcome {
    let Some(version) = Version::from_tag(&release.tag) else {
        return CheckOutcome::UpToDate { latest: None };
    };
    if release.draft || release.prerelease {
        return CheckOutcome::UpToDate { latest: None };
    }
    if version <= current {
        return CheckOutcome::UpToDate {
            latest: Some(version),
        };
    }
    let find = |name: &str| release.assets.iter().find(|a| a.name == name);
    let (zip, sig) = (zip_name(version), sig_name(version));
    let (Some(zip_asset), Some(sig_asset)) = (find(&zip), find(&sig)) else {
        return CheckOutcome::NotInstallable {
            version,
            reason: format!("its release has no {zip} with {sig}"),
        };
    };
    if !is_https(&zip_asset.url) || !is_https(&sig_asset.url) {
        return CheckOutcome::NotInstallable {
            version,
            reason: "its update files are not served over https".into(),
        };
    }
    if zip_asset.size > MAX_ZIP_BYTES {
        return CheckOutcome::NotInstallable {
            version,
            reason: "its update is larger than 50 MB".into(),
        };
    }
    CheckOutcome::Available(Update {
        version,
        notes: trim_notes(&plain_notes(&release.notes)),
        page_url: release_page(&release.page_url),
        zip_url: zip_asset.url.clone(),
        sig_url: sig_asset.url.clone(),
    })
}

/// Whether to offer `version`: automatic checks leave out the skipped
/// version, a manual check offers it again.
pub fn should_offer(version: Version, skipped: Option<Version>, manual: bool) -> bool {
    manual || skipped != Some(version)
}

/// Release notes written in Markdown, as plain text for the update alert
/// (L11). Headings lose their `#` marks, list items start with a bullet,
/// lines wrapped inside a paragraph or list item are joined, code fence
/// lines are dropped (their content stays as is), and inline marks (`**`,
/// `__`, backticks) and link targets are removed. Anything else passes
/// through.
pub fn plain_notes(markdown: &str) -> String {
    let text = markdown.replace("\r\n", "\n").replace('\r', "\n");
    let mut out: Vec<String> = Vec::new();
    let mut block: Option<String> = None;
    let mut in_fence = false;
    let flush = |block: &mut Option<String>, out: &mut Vec<String>| {
        if let Some(text) = block.take() {
            out.push(text);
        }
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            flush(&mut block, &mut out);
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            out.push(raw.trim_end().to_string());
            continue;
        }
        if line.is_empty() {
            flush(&mut block, &mut out);
            out.push(String::new());
        } else if let Some(heading) = markdown_heading(line) {
            flush(&mut block, &mut out);
            out.push(plain_inline(heading));
        } else if let Some(item) = markdown_list_item(line) {
            flush(&mut block, &mut out);
            block = Some(item);
        } else {
            let text = plain_inline(line);
            match &mut block {
                Some(current) => {
                    current.push(' ');
                    current.push_str(&text);
                }
                None => block = Some(text),
            }
        }
    }
    flush(&mut block, &mut out);
    out.join("\n")
}

/// The text of an ATX heading (`#` to `######`, then a space).
fn markdown_heading(line: &str) -> Option<&str> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = line[level..].strip_prefix(' ')?;
    Some(rest.trim().trim_end_matches('#').trim_end())
}

/// A list item as "• text", or "N. text" for a numbered one.
fn markdown_list_item(line: &str) -> Option<String> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(format!("\u{2022} {}", plain_inline(rest.trim_start())));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let rest = line[digits..].strip_prefix(". ")?;
    (digits > 0).then(|| format!("{}. {}", &line[..digits], plain_inline(rest.trim_start())))
}

/// Inline Markdown as plain text: `**` and `__` and backticks removed,
/// `[text](url)` and `![text](url)` reduced to the text.
fn plain_inline(text: &str) -> String {
    let text = text.replace("**", "").replace("__", "").replace('`', "");
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(open) = rest.find('[') {
        let label_end = rest[open..].find("](").map(|i| open + i);
        let link_end = label_end.and_then(|end| rest[end..].find(')').map(|i| end + i));
        let (Some(label_end), Some(link_end)) = (label_end, link_end) else {
            break;
        };
        let before = &rest[..open];
        out.push_str(before.strip_suffix('!').unwrap_or(before));
        out.push_str(&rest[open + 1..label_end]);
        rest = &rest[link_end + 1..];
    }
    out.push_str(rest);
    out
}

/// Release notes as plain text of at most `NOTES_MAX_CHARS` characters:
/// line endings unified, control characters dropped, trailing spaces and
/// runs of blank lines removed, cut with "..." when too long.
pub fn trim_notes(body: &str) -> String {
    let unified = body.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    let mut blank_run = 0;
    for line in unified.lines() {
        let line: String = line
            .chars()
            .filter(|c| *c == '\t' || !c.is_control())
            .collect();
        let line = line.trim_end();
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    let out = out.trim();
    if out.chars().count() <= NOTES_MAX_CHARS {
        return out.to_string();
    }
    let cut: String = out.chars().take(NOTES_MAX_CHARS - 3).collect();
    format!("{}...", cut.trim_end())
}

/// Whether an automatic check is due. All times are wall clock. Due when
/// the setting is on, no check is running, launch was at least
/// `FIRST_CHECK_DELAY` ago, the last successful check is missing, at least
/// `CHECK_INTERVAL` old or in the future (the clock moved back), and no
/// attempt in this process started within `ATTEMPT_SPACING`. An attempt
/// time in the future counts as no attempt.
pub fn check_due(
    now: SystemTime,
    launched_at: SystemTime,
    last_success: Option<SystemTime>,
    last_attempt: Option<SystemTime>,
    enabled: bool,
    running: bool,
) -> bool {
    if !enabled || running {
        return false;
    }
    let age = |t: SystemTime| now.duration_since(t).ok();
    if age(launched_at).is_none_or(|d| d < FIRST_CHECK_DELAY) {
        return false;
    }
    let stale = match last_success.map(age) {
        None | Some(None) => true,
        Some(Some(d)) => d >= CHECK_INTERVAL,
    };
    let attempted_recently = last_attempt
        .and_then(age)
        .is_some_and(|d| d < ATTEMPT_SPACING);
    stale && !attempted_recently
}

/// Whole seconds since the Unix epoch, 0 before it.
pub fn epoch_seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The time `seconds` after the Unix epoch, `None` for negative or
/// non-finite values.
pub fn from_epoch_seconds(seconds: f64) -> Option<SystemTime> {
    (seconds.is_finite() && seconds >= 0.0)
        .then(|| UNIX_EPOCH.checked_add(Duration::from_secs_f64(seconds)))
        .flatten()
}

/// Whether the first launch of this version opens Settings on the
/// Permissions page: the version differs from `LastLaunchedVersion` (a
/// missing value differs too) and a permission is missing. A build with
/// another signing requirement, such as an ad-hoc build before 0.1.5, does
/// not match the grants of the previous one.
pub fn needs_permission_notice(
    last_launched: Option<&str>,
    current: &str,
    screen_recording: bool,
    can_post_events: bool,
) -> bool {
    last_launched != Some(current) && !(screen_recording && can_post_events)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyError {
    PlaceholderKey,
    InvalidKey,
    MalformedSignature,
    Mismatch,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            VerifyError::PlaceholderKey => "this build has no update signing key",
            VerifyError::InvalidKey => "the update public key is not a valid Ed25519 key",
            VerifyError::MalformedSignature => {
                "the signature file is not one line of base64 holding 64 bytes"
            }
            VerifyError::Mismatch => "the signature does not match the update zip",
        })
    }
}

impl std::error::Error for VerifyError {}

/// Reads a `.sig` file: one line of standard base64 holding 64 bytes. A
/// final line break is allowed.
pub fn parse_signature(text: &str) -> Result<Signature, VerifyError> {
    let line = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    let bytes: [u8; 64] = base64::decode(line)
        .and_then(|b| b.try_into().ok())
        .ok_or(VerifyError::MalformedSignature)?;
    Ok(Signature::from_bytes(&bytes))
}

/// Verifies `signature_text` over the exact `zip` bytes. Strict
/// verification also refuses weak keys and non-canonical signatures.
pub fn verify_zip(
    public_key: &[u8; 32],
    zip: &[u8],
    signature_text: &str,
) -> Result<(), VerifyError> {
    if is_placeholder_key(public_key) {
        return Err(VerifyError::PlaceholderKey);
    }
    let key = VerifyingKey::from_bytes(public_key).map_err(|_| VerifyError::InvalidKey)?;
    let signature = parse_signature(signature_text)?;
    key.verify_strict(zip, &signature)
        .map_err(|_| VerifyError::Mismatch)
}

/// The `.sig` file content for `zip`: base64 and a line break.
pub fn sign_zip(secret: &[u8; 32], zip: &[u8]) -> String {
    let signature = SigningKey::from_bytes(secret).sign(zip);
    format!("{}\n", base64::encode(&signature.to_bytes()))
}

/// The public key that belongs to a private key.
pub fn public_key_of(secret: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(secret).verifying_key().to_bytes()
}

/// A 32-byte key as one line of base64, the format of the private key
/// file and of printed public keys.
pub fn encode_key(key: &[u8; 32]) -> String {
    base64::encode(key)
}

/// Reads a key written by `encode_key`; a final line break is allowed.
pub fn parse_key(text: &str) -> Option<[u8; 32]> {
    let line = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    base64::decode(line)?.try_into().ok()
}

/// The bundle that holds `exe`, when it sits at `<X>.app/Contents/MacOS/`.
pub fn bundle_of_executable(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let fits = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app";
    fits.then(|| bundle.to_path_buf())
}

/// Whether macOS runs the app from an App Translocation mount, a read-only
/// copy made for a quarantined app opened where it was downloaded.
pub fn is_translocated(path: &Path) -> bool {
    path.to_string_lossy().contains("/AppTranslocation/")
}

/// `<name>.app.previous` next to the bundle.
pub fn previous_path(bundle: &Path) -> Option<PathBuf> {
    let mut name = bundle.file_name()?.to_os_string();
    name.push(".previous");
    Some(bundle.with_file_name(name))
}

/// Name prefix of staging directories next to the bundle.
pub const STAGING_PREFIX: &str = ".IceRust.update.";

/// The `mkdtemp` template for a staging directory in `parent`.
pub fn staging_template(parent: &Path) -> PathBuf {
    parent.join(format!("{STAGING_PREFIX}XXXXXX"))
}

/// Whether a directory name is a staging directory.
pub fn is_staging_name(name: &str) -> bool {
    name.strip_prefix(STAGING_PREFIX)
        .is_some_and(|rest| !rest.is_empty())
}

/// Why an update cannot be installed in place. Checked before the prompt
/// offers Install and again when the install starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrecheckError {
    NoSigningKey,
    NotInstalled,
    NoWriteAccess { dir: PathBuf, version: Version },
}

impl fmt::Display for PrecheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrecheckError::NoSigningKey => f.write_str(
                "This build of ice-rust cannot verify updates. Download the new version from \
                 the release page.",
            ),
            PrecheckError::NotInstalled => {
                f.write_str("Move IceRust.app to the Applications folder to install updates")
            }
            PrecheckError::NoWriteAccess { dir, version } => write!(
                f,
                "Updating needs write access to {}. Ask an administrator to install ice-rust \
                 {version}, or move IceRust.app to a folder you own.",
                dir.display()
            ),
        }
    }
}

impl std::error::Error for PrecheckError {}

/// Checks that `bundle` can take `version` in place. `writable` is
/// `access(2)` with `W_OK` in the app; tests inject it.
pub fn precheck(
    bundle: Option<&Path>,
    key: &[u8; 32],
    version: Version,
    writable: impl Fn(&Path) -> bool,
) -> Result<PathBuf, PrecheckError> {
    if is_placeholder_key(key) {
        return Err(PrecheckError::NoSigningKey);
    }
    let bundle = bundle
        .filter(|b| b.extension().is_some_and(|e| e == "app") && !is_translocated(b))
        .ok_or(PrecheckError::NotInstalled)?;
    let parent = bundle
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(PrecheckError::NotInstalled)?;
    for dir in [parent, bundle] {
        if !writable(dir) {
            return Err(PrecheckError::NoWriteAccess {
                dir: dir.to_path_buf(),
                version,
            });
        }
    }
    Ok(bundle.to_path_buf())
}

/// The extracted zip must hold exactly one entry, `IceRust.app`.
pub fn check_extracted(names: &[String]) -> Result<(), String> {
    match names {
        [only] if only == BUNDLE_NAME => Ok(()),
        [] => Err("the update zip is empty".into()),
        _ => Err(format!(
            "the update zip must hold only {BUNDLE_NAME}, found: {}",
            names.join(", ")
        )),
    }
}

/// The new bundle's identifier and short version must be ours and the
/// offered version.
pub fn check_bundle_info(
    identifier: Option<&str>,
    version: Option<&str>,
    expected: Version,
) -> Result<(), String> {
    if identifier != Some(BUNDLE_ID) {
        return Err(format!(
            "the update's bundle identifier is {}, not {BUNDLE_ID}",
            identifier.unwrap_or("missing")
        ));
    }
    if version.and_then(Version::parse) != Some(expected) {
        return Err(format!(
            "the update's bundle version is {}, not {expected}",
            version.unwrap_or("missing")
        ));
    }
    Ok(())
}

/// An update never replaces an equal or newer installed version.
pub fn check_newer(installed: Option<&str>, offered: Version) -> Result<(), String> {
    match installed.and_then(Version::parse) {
        Some(current) if offered > current => Ok(()),
        Some(current) => Err(format!(
            "ice-rust {offered} is not newer than the installed {current}"
        )),
        None => Err("cannot read the installed bundle's version".into()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwapError {
    /// Renaming the current bundle aside failed; nothing changed.
    MoveOld(String),
    /// Renaming the new bundle into place failed. `restored` says whether
    /// the previous bundle went back.
    MoveNew {
        error: String,
        restored: Result<(), String>,
        previous: PathBuf,
    },
}

impl fmt::Display for SwapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SwapError::MoveOld(err) => {
                write!(f, "could not move the current bundle aside ({err})")
            }
            SwapError::MoveNew {
                error,
                restored: Ok(()),
                ..
            } => write!(
                f,
                "could not move the new bundle into place ({error}); the current version was \
                 put back"
            ),
            SwapError::MoveNew {
                error,
                restored: Err(again),
                previous,
            } => write!(
                f,
                "could not move the new bundle into place ({error}), and putting the current \
                 version back failed too ({again}); it is at {}",
                previous.display()
            ),
        }
    }
}

impl std::error::Error for SwapError {}

/// Swaps `new_bundle` in for `bundle` with two renames in the same
/// directory, keeping the old one at `previous`. When the second rename
/// fails the first is undone.
pub fn swap(
    bundle: &Path,
    new_bundle: &Path,
    previous: &Path,
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), SwapError> {
    rename(bundle, previous).map_err(|e| SwapError::MoveOld(e.to_string()))?;
    if let Err(error) = rename(new_bundle, bundle) {
        let restored = rename(previous, bundle).map_err(|e| e.to_string());
        return Err(SwapError::MoveNew {
            error: error.to_string(),
            restored,
            previous: previous.to_path_buf(),
        });
    }
    Ok(())
}

/// Shell for the relaunch helper.
pub const RELAUNCH_SHELL: &str = "/bin/sh";
/// Waits up to 60 s for pid `$1` to exit, then opens bundle `$2`. Both
/// arrive as positional arguments, never inside the script text.
pub const RELAUNCH_SCRIPT: &str = "i=0; while kill -0 \"$1\" 2>/dev/null; do i=$((i + 1)); \
     if [ \"$i\" -ge 300 ]; then exit 1; fi; sleep 0.2; done; exec /usr/bin/open \"$2\"";

/// Arguments for `RELAUNCH_SHELL`: the script, `$0`, the pid and the path.
pub fn relaunch_args(pid: u32, bundle: &Path) -> Vec<OsString> {
    vec![
        "-c".into(),
        RELAUNCH_SCRIPT.into(),
        "ice-rust-relaunch".into(),
        pid.to_string().into(),
        bundle.as_os_str().to_os_string(),
    ]
}

/// Path of the curl binary; never looked up through `PATH`.
pub const CURL: &str = "/usr/bin/curl";

fn curl_common(max_size: &str, max_time: &str) -> Vec<OsString> {
    [
        "--fail",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--max-filesize",
        max_size,
        "--max-time",
        max_time,
        "--silent",
        "--show-error",
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

/// curl arguments that download `url` into `out`.
pub fn curl_download_args(url: &str, out: &Path) -> Vec<OsString> {
    let mut args = curl_common("50M", "120");
    args.push("--output".into());
    args.push(out.as_os_str().to_os_string());
    args.push(url.into());
    args
}

/// curl arguments that print the release feed at `url`.
pub fn curl_feed_args(url: &str) -> Vec<OsString> {
    let mut args = curl_common("1M", "30");
    args.push("--header".into());
    args.push("Accept: application/vnd.github+json".into());
    args.push("--user-agent".into());
    args.push(format!("ice-rust/{CURRENT_VERSION}").into());
    args.push(url.into());
    args
}

/// Where the feed comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedSource {
    Https(String),
    /// A local JSON file, for tests in debug builds.
    File(PathBuf),
}

/// The feed from the `ICE_RUST_UPDATE_FEED` override, or the GitHub feed.
/// `allow_file` is true only in debug builds.
pub fn feed_source(override_url: Option<&str>, allow_file: bool) -> Result<FeedSource, String> {
    let Some(url) = override_url.filter(|u| !u.is_empty()) else {
        return Ok(FeedSource::Https(FEED_URL.to_string()));
    };
    if is_https(url) {
        return Ok(FeedSource::Https(url.to_string()));
    }
    match url.strip_prefix("file://") {
        Some(path) if allow_file && path.starts_with('/') => Ok(FeedSource::File(path.into())),
        Some(_) if allow_file => Err(format!("{FEED_ENV} must be an absolute file:// URL")),
        _ if allow_file => Err(format!("{FEED_ENV} must be an https:// or file:// URL")),
        _ => Err(format!(
            "{FEED_ENV} must be an https:// URL in a release build"
        )),
    }
}

/// What the updater is doing, for the About page and the icon menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Available(Version),
    NotInstallable(Version, String),
    CheckFailed(String),
    Downloading(Version),
    Installing(Version),
    InstallFailed(String),
    Relaunching(Version),
}

impl UpdateStatus {
    /// A check or an install is under way.
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            UpdateStatus::Checking
                | UpdateStatus::Downloading(_)
                | UpdateStatus::Installing(_)
                | UpdateStatus::Relaunching(_)
        )
    }
}

/// The About page's status line. `last_checked` is the formatted time of
/// the last successful check.
pub fn status_line(status: &UpdateStatus, last_checked: Option<&str>) -> String {
    match status {
        UpdateStatus::Idle => match last_checked {
            Some(time) => format!("Last checked {time}"),
            None => "Not checked for updates yet".into(),
        },
        UpdateStatus::Checking => "Checking for updates...".into(),
        UpdateStatus::UpToDate => MSG_UP_TO_DATE.into(),
        UpdateStatus::Available(v) => format!("ice-rust {v} is available"),
        UpdateStatus::NotInstallable(v, reason) => {
            format!("ice-rust {v} is out, but {reason}; see the release page")
        }
        UpdateStatus::CheckFailed(err) => format!("Could not check for updates: {err}"),
        UpdateStatus::Downloading(v) => format!("Downloading ice-rust {v}..."),
        UpdateStatus::Installing(v) => format!("Installing ice-rust {v}..."),
        UpdateStatus::InstallFailed(err) => format!("The update failed: {err}"),
        UpdateStatus::Relaunching(v) => format!("Relaunching ice-rust {v}..."),
    }
}

/// The prompt's message text.
pub fn prompt_title(new: Version, current: Version) -> String {
    format!("ice-rust {new} is available (you have {current})")
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    fn v(major: u64, minor: u64, patch: u64) -> Version {
        Version::new(major, minor, patch)
    }

    /// The shape of `GET /repos/{owner}/{repo}/releases/latest`, trimmed
    /// of fields the updater does not read except a few for realism.
    fn fixture(tag: &str, assets: &[(&str, &str, u64)]) -> String {
        let assets: Vec<String> = assets
            .iter()
            .enumerate()
            .map(|(i, (name, url, size))| {
                format!(
                    r#"{{"url":"https://api.github.com/repos/UnkAtreus/ice-rust/releases/assets/{i}",
                    "id":{i},"node_id":"RA_x","name":"{name}","label":"",
                    "uploader":{{"login":"UnkAtreus","id":1,"type":"User","site_admin":false}},
                    "content_type":"application/zip","state":"uploaded","size":{size},
                    "digest":"sha256:00","download_count":3,
                    "created_at":"2026-10-14T00:00:00Z","updated_at":"2026-10-14T00:00:00Z",
                    "browser_download_url":"{url}"}}"#
                )
            })
            .collect();
        format!(
            r###"{{"url":"https://api.github.com/repos/UnkAtreus/ice-rust/releases/1",
            "assets_url":"https://api.github.com/repos/UnkAtreus/ice-rust/releases/1/assets",
            "upload_url":"https://uploads.github.com/repos/UnkAtreus/ice-rust/releases/1/assets{{?name,label}}",
            "html_url":"https://github.com/UnkAtreus/ice-rust/releases/tag/{tag}",
            "id":1,"author":{{"login":"UnkAtreus","id":1}},"node_id":"RE_x",
            "tag_name":"{tag}","target_commitish":"main","name":"ice-rust {tag}",
            "draft":false,"immutable":false,"prerelease":false,
            "created_at":"2026-10-14T00:00:00Z","published_at":"2026-10-14T00:00:00Z",
            "assets":[{}],
            "tarball_url":"https://api.github.com/repos/UnkAtreus/ice-rust/tarball/{tag}",
            "zipball_url":"https://api.github.com/repos/UnkAtreus/ice-rust/zipball/{tag}",
            "body":"## Changes\r\n\r\n- Launch at login\r\n- Automatic updates\r\n"}}"###,
            assets.join(",")
        )
    }

    fn release_020() -> String {
        fixture(
            "v0.2.0",
            &[
                (
                    "IceRust-0.2.0-arm64.zip",
                    "https://github.com/UnkAtreus/ice-rust/releases/download/v0.2.0/IceRust-0.2.0-arm64.zip",
                    4_200_000,
                ),
                (
                    "IceRust-0.2.0-arm64.zip.sha256",
                    "https://github.com/UnkAtreus/ice-rust/releases/download/v0.2.0/IceRust-0.2.0-arm64.zip.sha256",
                    90,
                ),
                (
                    "IceRust-0.2.0-arm64.zip.sig",
                    "https://github.com/UnkAtreus/ice-rust/releases/download/v0.2.0/IceRust-0.2.0-arm64.zip.sig",
                    89,
                ),
            ],
        )
    }

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(Version::parse("0.1.0"), Some(v(0, 1, 0)));
        assert_eq!(Version::parse("10.20.30"), Some(v(10, 20, 30)));
        assert_eq!(Version::from_tag("v1.2.3"), Some(v(1, 2, 3)));
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.02.3",
            "1.2.3-beta",
            "1.2.3+b",
            "v1.2.3",
            " 1.2.3",
            "1.2.x",
            "1..3",
            "-1.2.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
        assert_eq!(Version::from_tag("1.2.3"), None);
        assert_eq!(Version::from_tag("V1.2.3"), None);
        assert!(v(0, 1, 10) > v(0, 1, 9));
        assert!(v(0, 2, 0) > v(0, 1, 99));
        assert!(v(1, 0, 0) > v(0, 99, 99));
        assert_eq!(v(1, 2, 3).to_string(), "1.2.3");
    }

    #[test]
    fn current_version_parses() {
        assert_eq!(
            Some(Version::current()),
            Version::parse(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn asset_names() {
        assert_eq!(zip_name(v(0, 2, 0)), "IceRust-0.2.0-arm64.zip");
        assert_eq!(sig_name(v(0, 2, 0)), "IceRust-0.2.0-arm64.zip.sig");
        assert_eq!(
            version_from_zip_name("IceRust-1.4.10-arm64.zip"),
            Some(v(1, 4, 10))
        );
        assert_eq!(version_from_zip_name("IceRust-1.4-arm64.zip"), None);
        assert_eq!(version_from_zip_name("IceRust-1.4.0-x86_64.zip"), None);
        assert_eq!(version_from_zip_name("update.zip"), None);
    }

    #[test]
    fn parses_the_release_fixture() {
        let release = parse_release(&release_020()).unwrap();
        assert_eq!(release.tag, "v0.2.0");
        assert!(!release.draft && !release.prerelease);
        assert_eq!(
            release.page_url,
            "https://github.com/UnkAtreus/ice-rust/releases/tag/v0.2.0"
        );
        assert_eq!(release.assets.len(), 3);
        assert_eq!(release.assets[0].size, 4_200_000);
        assert!(release.notes.starts_with("## Changes"));
    }

    #[test]
    fn feed_errors() {
        assert!(matches!(parse_release("not json"), Err(FeedError::Json(_))));
        assert_eq!(
            parse_release(r#"{"message":"Not Found"}"#),
            Err(FeedError::Missing("tag_name"))
        );
        assert_eq!(
            parse_release(r#"{"tag_name":"v1.0.0"}"#),
            Err(FeedError::Missing("assets"))
        );
        // An asset without a download URL is left out.
        let release =
            parse_release(r#"{"tag_name":"v1.0.0","assets":[{"name":"x.zip"}]}"#).unwrap();
        assert!(release.assets.is_empty());
        assert_eq!(release.notes, "");
    }

    #[test]
    fn offers_a_newer_release_with_both_assets() {
        let release = parse_release(&release_020()).unwrap();
        let CheckOutcome::Available(update) = evaluate(v(0, 1, 0), &release) else {
            panic!("expected an update");
        };
        assert_eq!(update.version, v(0, 2, 0));
        assert!(update.zip_url.ends_with("/IceRust-0.2.0-arm64.zip"));
        assert!(update.sig_url.ends_with("/IceRust-0.2.0-arm64.zip.sig"));
        assert_eq!(
            update.notes,
            "Changes\n\n\u{2022} Launch at login\n\u{2022} Automatic updates"
        );
    }

    #[test]
    fn never_offers_an_equal_or_lower_version() {
        let release = parse_release(&release_020()).unwrap();
        for current in [v(0, 2, 0), v(0, 2, 1), v(1, 0, 0)] {
            assert_eq!(
                evaluate(current, &release),
                CheckOutcome::UpToDate {
                    latest: Some(v(0, 2, 0))
                }
            );
        }
    }

    #[test]
    fn needs_both_assets_over_https() {
        let only_zip = fixture(
            "v0.2.0",
            &[(
                "IceRust-0.2.0-arm64.zip",
                "https://github.com/x/IceRust-0.2.0-arm64.zip",
                10,
            )],
        );
        let release = parse_release(&only_zip).unwrap();
        assert!(matches!(
            evaluate(v(0, 1, 0), &release),
            CheckOutcome::NotInstallable { version, .. } if version == v(0, 2, 0)
        ));
        // Assets of another version do not count.
        let wrong = fixture(
            "v0.2.0",
            &[
                ("IceRust-0.1.9-arm64.zip", "https://x/a.zip", 10),
                ("IceRust-0.1.9-arm64.zip.sig", "https://x/a.zip.sig", 10),
            ],
        );
        assert!(matches!(
            evaluate(v(0, 1, 0), &parse_release(&wrong).unwrap()),
            CheckOutcome::NotInstallable { .. }
        ));
        let http = fixture(
            "v0.2.0",
            &[
                ("IceRust-0.2.0-arm64.zip", "http://x/a.zip", 10),
                ("IceRust-0.2.0-arm64.zip.sig", "https://x/a.zip.sig", 10),
            ],
        );
        assert!(matches!(
            evaluate(v(0, 1, 0), &parse_release(&http).unwrap()),
            CheckOutcome::NotInstallable { reason, .. } if reason.contains("https")
        ));
        let huge = fixture(
            "v0.2.0",
            &[
                (
                    "IceRust-0.2.0-arm64.zip",
                    "https://x/a.zip",
                    MAX_ZIP_BYTES + 1,
                ),
                ("IceRust-0.2.0-arm64.zip.sig", "https://x/a.zip.sig", 10),
            ],
        );
        assert!(matches!(
            evaluate(v(0, 1, 0), &parse_release(&huge).unwrap()),
            CheckOutcome::NotInstallable { .. }
        ));
    }

    #[test]
    fn odd_tags_drafts_and_prereleases_are_not_offered() {
        let mut release = parse_release(&release_020()).unwrap();
        release.prerelease = true;
        assert_eq!(
            evaluate(v(0, 1, 0), &release),
            CheckOutcome::UpToDate { latest: None }
        );
        release.prerelease = false;
        release.draft = true;
        assert_eq!(
            evaluate(v(0, 1, 0), &release),
            CheckOutcome::UpToDate { latest: None }
        );
        release.draft = false;
        release.tag = "nightly".into();
        assert_eq!(
            evaluate(v(0, 1, 0), &release),
            CheckOutcome::UpToDate { latest: None }
        );
    }

    #[test]
    fn release_page_stays_in_the_repository() {
        let mut release = parse_release(&release_020()).unwrap();
        release.page_url = "https://example.com/phish".into();
        let CheckOutcome::Available(update) = evaluate(v(0, 1, 0), &release) else {
            panic!("expected an update");
        };
        assert_eq!(update.page_url, RELEASES_URL);
    }

    #[test]
    fn skipped_version_is_offered_only_on_a_manual_check() {
        assert!(!should_offer(v(0, 2, 0), Some(v(0, 2, 0)), false));
        assert!(should_offer(v(0, 2, 0), Some(v(0, 2, 0)), true));
        assert!(should_offer(v(0, 2, 1), Some(v(0, 2, 0)), false));
        assert!(should_offer(v(0, 2, 0), None, false));
    }

    #[test]
    fn markdown_notes_become_plain_text() {
        let notes = "## Fixed\r\n\r\n- **Memory after closing Settings (L7).** AppKit kept the\r\n  closed window until a later event.\r\n- Second item\r\n\r\n### Files\r\n\r\n1. `IceRust-0.1.2-arm64.zip`: the app\r\n\r\n```\r\nshasum -a 256 -c x.sha256\r\n```\r\n\r\nA port of [Ice](https://github.com/jordanbaird/Ice) by\r\nJordan Baird. ![logo](https://x/y.png)";
        assert_eq!(
            plain_notes(notes),
            "Fixed\n\n\u{2022} Memory after closing Settings (L7). AppKit kept the closed window until a later event.\n\u{2022} Second item\n\nFiles\n\n1. IceRust-0.1.2-arm64.zip: the app\n\nshasum -a 256 -c x.sha256\n\nA port of Ice by Jordan Baird. logo"
        );
    }

    #[test]
    fn plain_notes_leave_other_text_alone() {
        assert_eq!(plain_notes(""), "");
        assert_eq!(plain_notes("#hashtag and 3.5 MB"), "#hashtag and 3.5 MB");
        assert_eq!(plain_notes("[not a link] (x)"), "[not a link] (x)");
        assert_eq!(plain_notes("a [b](c"), "a [b](c");
        assert_eq!(plain_notes("##"), "##");
        assert_eq!(plain_notes("* star item"), "\u{2022} star item");
        // Fence content keeps its own lines and marks.
        assert_eq!(plain_notes("```\n**x**\n  y\n```"), "**x**\n  y");
    }

    #[test]
    fn notes_are_trimmed_plain_text() {
        assert_eq!(trim_notes(""), "");
        assert_eq!(trim_notes("  \n\n "), "");
        assert_eq!(trim_notes("a  \r\n\r\n\r\n\r\nb\u{7}\tc\r"), "a\n\nb\tc");
        let long = "x".repeat(4000);
        let trimmed = trim_notes(&long);
        assert_eq!(trimmed.chars().count(), NOTES_MAX_CHARS);
        assert!(trimmed.ends_with("..."));
        // Multi-byte characters count once and are never split.
        let wide = "\u{e01}".repeat(2000);
        let trimmed = trim_notes(&wide);
        assert_eq!(trimmed.chars().count(), NOTES_MAX_CHARS);
        let exact = "y".repeat(NOTES_MAX_CHARS);
        assert_eq!(trim_notes(&exact), exact);
    }

    mod due {
        use super::*;

        /// Launch at a fixed wall-clock time.
        fn launch() -> SystemTime {
            UNIX_EPOCH + Duration::from_secs(1_800_000_000)
        }

        fn at(after_launch: Duration) -> SystemTime {
            launch() + after_launch
        }

        fn due(
            now: Duration,
            success: Option<SystemTime>,
            attempt: Option<SystemTime>,
            enabled: bool,
            running: bool,
        ) -> bool {
            check_due(at(now), launch(), success, attempt, enabled, running)
        }

        const MINUTE: Duration = Duration::from_secs(60);

        #[test]
        fn first_check_without_last_update_check() {
            assert!(due(MINUTE, None, None, true, false));
        }

        #[test]
        fn not_before_launch_plus_60_s() {
            assert!(!due(Duration::from_secs(59), None, None, true, false));
            assert!(!due(Duration::ZERO, None, None, true, false));
        }

        #[test]
        fn recent_check_means_no_request_at_launch() {
            // Checked 2 h before this launch: the next check waits for 24 h.
            let success = Some(launch() - 2 * HOUR);
            assert!(!due(MINUTE, success, None, true, false));
            assert!(!due(21 * HOUR, success, None, true, false));
            assert!(due(22 * HOUR, success, None, true, false));
        }

        #[test]
        fn stale_check_is_due() {
            let success = Some(launch() - 25 * HOUR);
            assert!(due(MINUTE, success, None, true, false));
            assert!(due(
                MINUTE,
                Some(launch() - 24 * HOUR + MINUTE),
                None,
                true,
                false
            ));
        }

        #[test]
        fn clock_moved_backwards() {
            // LastUpdateCheck is later than now: treated as missing.
            let success = Some(launch() + 30 * 24 * HOUR);
            assert!(due(MINUTE, success, None, true, false));
            // An attempt in the future does not block either.
            let attempt = Some(launch() + 5 * HOUR);
            assert!(due(MINUTE, None, attempt, true, false));
            // Now before launch: the 60 s wait cannot be confirmed.
            assert!(!check_due(
                launch() - HOUR,
                launch(),
                None,
                None,
                true,
                false
            ));
        }

        #[test]
        fn setting_off() {
            assert!(!due(10 * HOUR, None, None, false, false));
        }

        #[test]
        fn check_already_running() {
            assert!(!due(10 * HOUR, None, None, true, true));
        }

        #[test]
        fn failed_attempt_within_the_last_hour() {
            let attempt = Some(at(MINUTE));
            assert!(!due(MINUTE + 59 * MINUTE, None, attempt, true, false));
            assert!(due(MINUTE + HOUR, None, attempt, true, false));
        }

        #[test]
        fn epoch_seconds_round_trip() {
            let t = UNIX_EPOCH + Duration::from_secs(1_760_000_000);
            assert_eq!(epoch_seconds(t), 1_760_000_000);
            assert_eq!(from_epoch_seconds(1_760_000_000.0), Some(t));
            assert_eq!(from_epoch_seconds(-1.0), None);
            assert_eq!(from_epoch_seconds(f64::NAN), None);
            assert_eq!(epoch_seconds(UNIX_EPOCH - HOUR), 0);
        }
    }

    #[test]
    fn permission_notice_on_first_launch_of_a_version() {
        // Updated and a grant no longer applies.
        assert!(needs_permission_notice(Some("0.1.0"), "0.2.0", false, true));
        assert!(needs_permission_notice(Some("0.1.0"), "0.2.0", true, false));
        // Nothing stored (first launch, or an update from 0.1.0).
        assert!(needs_permission_notice(None, "0.2.0", false, false));
        // Same version, or every permission in place.
        assert!(!needs_permission_notice(
            Some("0.2.0"),
            "0.2.0",
            false,
            false
        ));
        assert!(!needs_permission_notice(Some("0.1.0"), "0.2.0", true, true));
    }

    mod signatures {
        use super::*;

        #[test]
        fn shipped_key_is_set_and_valid() {
            assert!(!is_placeholder_key(&UPDATE_PUBLIC_KEY));
            assert!(VerifyingKey::from_bytes(&UPDATE_PUBLIC_KEY).is_ok());
        }

        const SECRET: [u8; 32] = [7; 32];
        const OTHER: [u8; 32] = [9; 32];
        const ZIP: &[u8] = b"PK\x03\x04 pretend zip bytes";

        #[test]
        fn valid_signature_verifies() {
            let public = public_key_of(&SECRET);
            let sig = sign_zip(&SECRET, ZIP);
            assert!(sig.ends_with('\n'));
            assert_eq!(sig.trim_end().len(), 88);
            assert_eq!(verify_zip(&public, ZIP, &sig), Ok(()));
            assert_eq!(verify_zip(&public, ZIP, sig.trim_end()), Ok(()));
            assert_eq!(
                verify_zip(&public, ZIP, &format!("{}\r\n", sig.trim_end())),
                Ok(())
            );
        }

        #[test]
        fn tampered_zip_fails() {
            let public = public_key_of(&SECRET);
            let sig = sign_zip(&SECRET, ZIP);
            let mut tampered = ZIP.to_vec();
            tampered.push(0);
            assert_eq!(
                verify_zip(&public, &tampered, &sig),
                Err(VerifyError::Mismatch)
            );
            tampered.pop();
            tampered[0] ^= 1;
            assert_eq!(
                verify_zip(&public, &tampered, &sig),
                Err(VerifyError::Mismatch)
            );
        }

        #[test]
        fn wrong_key_fails() {
            let sig = sign_zip(&OTHER, ZIP);
            assert_eq!(
                verify_zip(&public_key_of(&SECRET), ZIP, &sig),
                Err(VerifyError::Mismatch)
            );
        }

        #[test]
        fn malformed_base64_fails() {
            let public = public_key_of(&SECRET);
            let sig = sign_zip(&SECRET, ZIP);
            let line = sig.trim_end();
            for bad in [
                String::new(),
                "not base64!".to_string(),
                line[..line.len() - 4].to_string(),
                format!("{line}\n{line}"),
                format!(" {line}"),
                base64::encode(&[0u8; 32]),
            ] {
                assert_eq!(
                    verify_zip(&public, ZIP, &bad),
                    Err(VerifyError::MalformedSignature),
                    "{bad:?}"
                );
            }
        }

        #[test]
        fn placeholder_key_is_refused() {
            let sig = sign_zip(&SECRET, ZIP);
            assert!(is_placeholder_key(&PLACEHOLDER_PUBLIC_KEY));
            assert!(!is_placeholder_key(&public_key_of(&SECRET)));
            assert_eq!(
                verify_zip(&PLACEHOLDER_PUBLIC_KEY, ZIP, &sig),
                Err(VerifyError::PlaceholderKey)
            );
        }

        #[test]
        fn keys_round_trip_as_base64() {
            let text = encode_key(&SECRET);
            assert_eq!(text.len(), 44);
            assert_eq!(parse_key(&text), Some(SECRET));
            assert_eq!(parse_key(&format!("{text}\n")), Some(SECRET));
            assert_eq!(parse_key("AAAA"), None);
            assert_eq!(parse_key(&format!(" {text}")), None);
        }
    }

    #[test]
    fn finds_the_bundle_of_an_executable() {
        assert_eq!(
            bundle_of_executable(Path::new(
                "/Applications/IceRust.app/Contents/MacOS/ice-rust"
            )),
            Some(PathBuf::from("/Applications/IceRust.app"))
        );
        assert_eq!(
            bundle_of_executable(Path::new("/Users/me/repo/target/release/ice-rust")),
            None
        );
        assert_eq!(
            bundle_of_executable(Path::new("/x/IceRust/Contents/MacOS/ice-rust")),
            None
        );
        assert_eq!(bundle_of_executable(Path::new("ice-rust")), None);
    }

    #[test]
    fn translocation_is_detected() {
        assert!(is_translocated(Path::new(
            "/private/var/folders/ab/xyz/T/AppTranslocation/1234-ABCD/d/IceRust.app"
        )));
        assert!(!is_translocated(Path::new("/Applications/IceRust.app")));
        assert!(!is_translocated(Path::new(
            "/Users/me/AppTranslocationX/IceRust.app"
        )));
    }

    #[test]
    fn install_paths() {
        assert_eq!(
            previous_path(Path::new("/Applications/IceRust.app")),
            Some(PathBuf::from("/Applications/IceRust.app.previous"))
        );
        assert_eq!(
            previous_path(Path::new("/Users/me/Apps/Ice Rust.app")),
            Some(PathBuf::from("/Users/me/Apps/Ice Rust.app.previous"))
        );
        assert_eq!(
            staging_template(Path::new("/Applications")),
            PathBuf::from("/Applications/.IceRust.update.XXXXXX")
        );
        assert!(is_staging_name(".IceRust.update.a1B2c3"));
        assert!(!is_staging_name(".IceRust.update."));
        assert!(!is_staging_name("IceRust.update.a1B2c3"));
        assert!(!is_staging_name("IceRust.app.previous"));
    }

    mod prechecks {
        use super::*;

        const KEY: [u8; 32] = [3; 32];
        const NEW: Version = Version::new(0, 2, 0);

        fn writable_except(denied: &'static str) -> impl Fn(&Path) -> bool {
            move |p: &Path| p != Path::new(denied)
        }

        #[test]
        fn passes_for_a_writable_bundle() {
            assert_eq!(
                precheck(
                    Some(Path::new("/Applications/IceRust.app")),
                    &KEY,
                    NEW,
                    |_| true
                ),
                Ok(PathBuf::from("/Applications/IceRust.app"))
            );
        }

        #[test]
        fn refuses_without_a_bundle_or_when_translocated() {
            assert_eq!(
                precheck(None, &KEY, NEW, |_| true),
                Err(PrecheckError::NotInstalled)
            );
            assert_eq!(
                precheck(
                    Some(Path::new("/Users/me/target/release")),
                    &KEY,
                    NEW,
                    |_| true
                ),
                Err(PrecheckError::NotInstalled)
            );
            let translocated =
                Path::new("/private/var/folders/x/T/AppTranslocation/ABC/d/IceRust.app");
            let err = precheck(Some(translocated), &KEY, NEW, |_| true).unwrap_err();
            assert_eq!(err, PrecheckError::NotInstalled);
            assert_eq!(
                err.to_string(),
                "Move IceRust.app to the Applications folder to install updates"
            );
        }

        #[test]
        fn refuses_without_write_access_and_names_the_folder() {
            let err = precheck(
                Some(Path::new("/Applications/IceRust.app")),
                &KEY,
                NEW,
                writable_except("/Applications"),
            )
            .unwrap_err();
            assert_eq!(
                err.to_string(),
                "Updating needs write access to /Applications. Ask an administrator to install \
                 ice-rust 0.2.0, or move IceRust.app to a folder you own."
            );
            let err = precheck(
                Some(Path::new("/Applications/IceRust.app")),
                &KEY,
                NEW,
                writable_except("/Applications/IceRust.app"),
            )
            .unwrap_err();
            assert!(matches!(err, PrecheckError::NoWriteAccess { dir, .. }
                if dir == Path::new("/Applications/IceRust.app")));
        }

        #[test]
        fn refuses_the_placeholder_key_first() {
            assert_eq!(
                precheck(None, &PLACEHOLDER_PUBLIC_KEY, NEW, |_| false),
                Err(PrecheckError::NoSigningKey)
            );
        }
    }

    #[test]
    fn extracted_zip_holds_one_bundle() {
        assert_eq!(check_extracted(&["IceRust.app".into()]), Ok(()));
        assert!(check_extracted(&[]).is_err());
        assert!(check_extracted(&["Other.app".into()]).is_err());
        assert!(check_extracted(&["IceRust.app".into(), "__MACOSX".into()]).is_err());
    }

    #[test]
    fn bundle_info_must_match() {
        let new = v(0, 2, 0);
        assert_eq!(
            check_bundle_info(Some(BUNDLE_ID), Some("0.2.0"), new),
            Ok(())
        );
        assert!(check_bundle_info(Some("com.example.Other"), Some("0.2.0"), new).is_err());
        assert!(check_bundle_info(None, Some("0.2.0"), new).is_err());
        assert!(check_bundle_info(Some(BUNDLE_ID), Some("0.2.1"), new).is_err());
        assert!(check_bundle_info(Some(BUNDLE_ID), None, new).is_err());
    }

    #[test]
    fn only_newer_versions_replace_the_installed_one() {
        assert_eq!(check_newer(Some("0.0.0"), v(0, 1, 0)), Ok(()));
        assert!(check_newer(Some("0.1.0"), v(0, 1, 0)).is_err());
        assert!(check_newer(Some("0.2.0"), v(0, 1, 0)).is_err());
        assert!(check_newer(None, v(0, 1, 0)).is_err());
        assert!(check_newer(Some("garbage"), v(0, 1, 0)).is_err());
    }

    mod swapping {
        use super::*;
        use std::cell::RefCell;

        type Log = RefCell<Vec<(PathBuf, PathBuf)>>;

        fn renamer(log: &Log, fail_on: &[usize]) -> impl FnMut(&Path, &Path) -> io::Result<()> {
            let fail_on = fail_on.to_vec();
            move |from: &Path, to: &Path| {
                let mut log = log.borrow_mut();
                log.push((from.to_path_buf(), to.to_path_buf()));
                if fail_on.contains(&(log.len() - 1)) {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            }
        }

        fn paths() -> (PathBuf, PathBuf, PathBuf) {
            (
                PathBuf::from("/A/IceRust.app"),
                PathBuf::from("/A/.IceRust.update.x/IceRust.app"),
                PathBuf::from("/A/IceRust.app.previous"),
            )
        }

        #[test]
        fn swaps_with_two_renames() {
            let (bundle, new, previous) = paths();
            let log = Log::default();
            assert_eq!(swap(&bundle, &new, &previous, renamer(&log, &[])), Ok(()));
            assert_eq!(
                log.into_inner(),
                vec![
                    (bundle.clone(), previous.clone()),
                    (new.clone(), bundle.clone())
                ]
            );
        }

        #[test]
        fn first_rename_failure_changes_nothing() {
            let (bundle, new, previous) = paths();
            let log = Log::default();
            let err = swap(&bundle, &new, &previous, renamer(&log, &[0])).unwrap_err();
            assert!(matches!(err, SwapError::MoveOld(_)));
            assert_eq!(log.into_inner().len(), 1);
        }

        #[test]
        fn second_rename_failure_rolls_back() {
            let (bundle, new, previous) = paths();
            let log = Log::default();
            let err = swap(&bundle, &new, &previous, renamer(&log, &[1])).unwrap_err();
            assert!(matches!(
                &err,
                SwapError::MoveNew {
                    restored: Ok(()),
                    ..
                }
            ));
            assert!(err.to_string().contains("put back"));
            assert_eq!(
                log.into_inner().last(),
                Some(&(previous.clone(), bundle.clone()))
            );
        }

        #[test]
        fn failed_rollback_names_the_previous_bundle() {
            let (bundle, new, previous) = paths();
            let log = Log::default();
            let err = swap(&bundle, &new, &previous, renamer(&log, &[1, 2])).unwrap_err();
            assert!(matches!(
                &err,
                SwapError::MoveNew {
                    restored: Err(_),
                    ..
                }
            ));
            assert!(err.to_string().contains("/A/IceRust.app.previous"));
        }
    }

    #[test]
    fn relaunch_helper_takes_positional_arguments() {
        let path = Path::new("/Users/me/Apps/Ice Rust\"; rm -rf ~; \".app");
        let args = relaunch_args(4242, path);
        assert_eq!(args.len(), 5);
        assert_eq!(args[0], "-c");
        assert_eq!(args[1], RELAUNCH_SCRIPT);
        assert_eq!(args[3], "4242");
        assert_eq!(args[4], path.as_os_str());
        // The script never contains the pid or the path.
        assert!(!RELAUNCH_SCRIPT.contains("4242"));
        assert!(!RELAUNCH_SCRIPT.contains("Ice Rust"));
        assert!(RELAUNCH_SCRIPT.contains("\"$1\"") && RELAUNCH_SCRIPT.contains("\"$2\""));
        assert!(RELAUNCH_SCRIPT.contains("exec /usr/bin/open"));
    }

    #[test]
    fn curl_arguments() {
        let args = curl_download_args("https://x/a.zip", Path::new("/s/a.zip"));
        let text: Vec<&str> = args.iter().filter_map(|a| a.to_str()).collect();
        for want in [
            "--fail",
            "--location",
            "--max-filesize",
            "50M",
            "--max-time",
            "120",
        ] {
            assert!(text.contains(&want), "{want}");
        }
        let proto = text.iter().position(|a| *a == "--proto").unwrap();
        assert_eq!(text[proto + 1], "=https");
        let redir = text.iter().position(|a| *a == "--proto-redir").unwrap();
        assert_eq!(text[redir + 1], "=https");
        assert_eq!(
            &text[text.len() - 3..],
            ["--output", "/s/a.zip", "https://x/a.zip"]
        );
        let feed = curl_feed_args(FEED_URL);
        assert_eq!(feed.last().map(|s| s.as_os_str()), Some(FEED_URL.as_ref()));
    }

    #[test]
    fn feed_override() {
        assert_eq!(
            feed_source(None, false),
            Ok(FeedSource::Https(FEED_URL.into()))
        );
        assert_eq!(
            feed_source(Some(""), true),
            Ok(FeedSource::Https(FEED_URL.into()))
        );
        assert_eq!(
            feed_source(Some("https://example.com/feed.json"), false),
            Ok(FeedSource::Https("https://example.com/feed.json".into()))
        );
        assert_eq!(
            feed_source(Some("file:///tmp/feed.json"), true),
            Ok(FeedSource::File("/tmp/feed.json".into()))
        );
        // Release builds accept https only.
        assert!(feed_source(Some("file:///tmp/feed.json"), false).is_err());
        assert!(feed_source(Some("http://example.com/feed.json"), false).is_err());
        assert!(feed_source(Some("http://example.com/feed.json"), true).is_err());
        assert!(feed_source(Some("file://tmp/feed.json"), true).is_err());
    }

    #[test]
    fn status_lines() {
        assert_eq!(
            status_line(&UpdateStatus::Idle, None),
            "Not checked for updates yet"
        );
        assert_eq!(
            status_line(&UpdateStatus::Idle, Some("Oct 14, 2026 at 9:30")),
            "Last checked Oct 14, 2026 at 9:30"
        );
        assert_eq!(
            status_line(&UpdateStatus::UpToDate, None),
            "ice-rust is up to date"
        );
        assert_eq!(
            status_line(&UpdateStatus::Available(v(0, 2, 0)), None),
            "ice-rust 0.2.0 is available"
        );
        assert!(UpdateStatus::Checking.is_busy());
        assert!(UpdateStatus::Downloading(v(0, 2, 0)).is_busy());
        assert!(!UpdateStatus::UpToDate.is_busy());
        assert_eq!(
            prompt_title(v(0, 2, 0), v(0, 1, 0)),
            "ice-rust 0.2.0 is available (you have 0.1.0)"
        );
    }
}
