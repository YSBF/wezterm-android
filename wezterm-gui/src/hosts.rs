//! SSH host profiles, and the path from one to a live connection.
//!
//! The sidebar needs somewhere to keep a list of hosts, and it cannot be
//! `wezterm.lua`: on a release build the config file is unreachable to the user
//! -- it lives in app-private storage and `run-as` is refused when the package
//! is not debuggable -- and rewriting it from the app would mean parsing and
//! regenerating Lua. So profiles are the app's own state, stored in a file it
//! owns.
//!
//! Because the UI is the only editor that file will ever have, an export and a
//! reset are part of the feature rather than a nicety. Export is a share intent
//! carrying the serialized list; see `export_document`, which excludes anything
//! secret.
//!
//! ## Which SSH
//!
//! There are two SSH paths in wezterm and they are not interchangeable:
//!
//! | `SshDomain.multiplexing` | Domain type       | Needs on the remote host    |
//! | ------------------------ | ----------------- | --------------------------- |
//! | `None`                   | `RemoteSshDomain` | nothing but an sshd         |
//! | `WezTerm`                | `ClientDomain`    | `wezterm cli proxy`         |
//!
//! A profile defaults to the first, because it is the one that works against
//! anything: the multiplexing flavour computes a `wezterm` path and executes it
//! on the far end, and fails against a plain server. `SshMultiplexing` itself
//! defaults to `WezTerm`, so this is a thing to get wrong by omission -- the
//! default here is set explicitly for that reason.
//!
//! The second is offered as an option because it buys the one thing the first
//! cannot: panes that outlive the app being closed. On a phone that matters more
//! than on a desktop, since Android stops backgrounded apps at its own
//! convenience. It costs a `wezterm-mux-server` on the far end whose
//! `CODEC_VERSION` matches this build's, which is why it is off unless asked
//! for, and why the failure says so rather than surfacing a codec mismatch.
//!
//! Connecting to a multiplexed profile *attaches* before it spawns. Spawning
//! unconditionally would reach a server holding the user's panes and then put a
//! new empty one in front of them, which is indistinguishable from having lost
//! the session.
//!
//! ## Why a domain name is never reused
//!
//! `Mux` has `add_domain` and `get_domain_by_name` but no removal, so a domain
//! lives for the life of the process, and registration keyed by name is
//! first-write-wins -- silently so, since the existing precedent skips a name it
//! already has. Connect to a host, edit its port, reconnect, and the second
//! connection would quietly use the first domain's configuration.
//!
//! A profile therefore carries a stable id and a generation counter, and the
//! domain name is derived from both, so an edited profile yields a name that has
//! never been registered. The sidebar shows `display_name`, so this stays
//! invisible.
//!
//! The cost is that dead domains accumulate for the life of the process. That is
//! accepted for now -- they are small, and the alternative is a removal API
//! upstream in `Mux` -- but it is a recorded decision rather than an accident,
//! and a long session that edits profiles repeatedly is the case to watch.

use crate::dialog::{ChoiceOption, DialogField, DialogSpec, DialogValues, FieldKind};
use anyhow::{anyhow, bail, Context};
use config::{ConfigHandle, SshDomain, SshMultiplexing};
use mux::domain::Domain;
use mux::Mux;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The default SSH port, used when a profile does not say.
pub const DEFAULT_PORT: u16 = 22;

/// The version stamped into the stored file.
///
/// A reader that finds a version it does not know refuses to load rather than
/// guessing, because guessing would mean overwriting the user's only copy of a
/// list it could not fully understand.
const FORMAT_VERSION: u32 = 1;

/// One stored SSH login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostProfile {
    /// Stable across edits, and never reused. The domain name is derived from
    /// this and `generation`.
    pub id: String,
    /// Bumped on every edit. See the module comment.
    #[serde(default)]
    pub generation: u32,
    /// What the sidebar shows.
    pub display_name: String,
    /// A host name or an IP address.
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub username: String,
    /// A private key inside app-private storage, if one was imported.
    ///
    /// Never a password: those are entered at connection time and are not stored
    /// at all. Any future persistent secret has to use the Android Keystore
    /// rather than a plaintext file.
    ///
    /// Superseded by `key_id` and kept for the files that already carry it, and
    /// as the way to name a key put in place by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_file: Option<PathBuf>,
    /// A key from the keychain, by id. Takes precedence over `key_file`.
    ///
    /// An id rather than a path so that one key can serve several hosts and be
    /// renamed without touching any of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// Whether to run panes inside `wezterm-mux-server` on the far end.
    ///
    /// Off by default, which is a plain ssh shell and needs nothing installed on
    /// the server. On, the panes outlive the app being closed -- but the server
    /// must have a `wezterm-mux-server` whose `CODEC_VERSION` matches this
    /// build's, and against a plain sshd it simply fails.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multiplexing: bool,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl HostProfile {
    /// A new profile with a freshly minted id.
    pub fn new(display_name: &str, host: &str, port: u16, username: &str) -> Self {
        Self {
            id: new_id(),
            generation: 0,
            display_name: display_name.trim().to_string(),
            host: host.trim().to_string(),
            port,
            username: username.trim().to_string(),
            key_file: None,
            key_id: None,
            multiplexing: false,
        }
    }

    /// The mux domain name for this profile at this generation.
    ///
    /// Derived rather than chosen, and never reused: see the module comment.
    pub fn domain_name(&self) -> String {
        format!("sshhost:{}:{}", self.id, self.generation)
    }

    /// The `SshDomain` this profile connects through.
    ///
    /// `key_path` is resolved by the caller, because a `key_id` means nothing
    /// without the keychain that defines it; see `HostRepository::key_path_for`.
    /// The multiplexing flavour follows `self.multiplexing`; see the module
    /// comment for what the two are and what each costs.
    pub fn to_ssh_domain(&self, key_path: Option<&Path>) -> SshDomain {
        SshDomain {
            name: self.domain_name(),
            // `ssh_domain_to_ssh_config` splits a trailing `:port` off this, and
            // that is the only channel it has for the port.
            remote_address: format!("{}:{}", self.host, self.port),
            username: Some(self.username.clone()),
            multiplexing: if self.multiplexing {
                SshMultiplexing::WezTerm
            } else {
                SshMultiplexing::None
            },
            ssh_option: self.ssh_options(key_path),
            ..SshDomain::default()
        }
    }

    fn ssh_options(&self, key_path: Option<&Path>) -> std::collections::HashMap<String, String> {
        let mut options = std::collections::HashMap::new();
        if let Some(key) = key_path {
            options.insert("identityfile".to_string(), key.display().to_string());
            // With a key of its own, do not let an agent or the user's other
            // keys be tried first: on a phone there is no agent, and offering
            // unrelated keys to a server is how an account gets locked out.
            options.insert("identitiesonly".to_string(), "yes".to_string());
        }
        options
    }

    /// Check the profile, returning every problem found rather than the first.
    ///
    /// All of them, because the editor shows them next to the fields and fixing
    /// one at a time through a dialog is miserable.
    pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = vec![];

        if self.display_name.trim().is_empty() {
            errors.push(ValidationError::new(Field::DisplayName, "cannot be empty"));
        } else if self.display_name.chars().any(|c| c.is_control()) {
            errors.push(ValidationError::new(
                Field::DisplayName,
                "cannot contain control characters",
            ));
        }

        if let Err(why) = validate_host(&self.host) {
            errors.push(ValidationError::new(Field::Host, why));
        }

        if self.port == 0 {
            errors.push(ValidationError::new(Field::Port, "must be 1 or greater"));
        }

        if let Err(why) = validate_username(&self.username) {
            errors.push(ValidationError::new(Field::Username, why));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Which field a validation error belongs to, so the editor can put the message
/// beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    DisplayName,
    Host,
    Port,
    Username,
}

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Self::DisplayName => "name",
            Self::Host => "host",
            Self::Port => "port",
            Self::Username => "user",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub field: Field,
    pub message: String,
}

impl ValidationError {
    fn new(field: Field, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field.label(), self.message)
    }
}

/// A host name or an IP address that ssh could actually be pointed at.
///
/// Rejecting rubbish here rather than at connection time matters because the
/// failure is otherwise a timeout: a host with a space in it produces a
/// resolution error twenty seconds later, which reads as an unreachable server
/// rather than as a typo.
fn validate_host(host: &str) -> Result<(), String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("cannot be empty".to_string());
    }
    if host.len() > 253 {
        return Err("is too long".to_string());
    }

    // An IPv6 literal, bracketed or bare, is a valid target and is not a DNS
    // name, so take it before the label rules below.
    let unbracketed = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    if unbracketed.parse::<IpAddr>().is_ok() {
        return Ok(());
    }

    if host.contains(':') {
        // The port lives in its own field. Accepting it here as well would mean
        // two places disagreeing about which one wins.
        return Err("must not include a port; use the port field".to_string());
    }
    if host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("must not contain spaces".to_string());
    }
    if host.contains('@') || host.contains('/') {
        return Err("must be a host name or an IP address only".to_string());
    }

    for label in host.split('.') {
        if label.is_empty() {
            return Err("has an empty part between dots".to_string());
        }
        if label.len() > 63 {
            return Err("has a part longer than 63 characters".to_string());
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err("has a part starting or ending with '-'".to_string());
        }
        if !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(
                "has a part with characters other than letters, digits and '-'".to_string(),
            );
        }
    }

    Ok(())
}

fn validate_username(username: &str) -> Result<(), String> {
    let username = username.trim();
    if username.is_empty() {
        return Err("cannot be empty".to_string());
    }
    if username
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("must not contain spaces".to_string());
    }
    // ssh parses `user@host`, and a colon would be read as the start of a port
    // by anything that later reassembles the two.
    if username.contains('@') || username.contains(':') {
        return Err("must not contain '@' or ':'".to_string());
    }
    Ok(())
}

/// A private key this app holds, and the name the user gave it.
///
/// The key material is *not* here: it lives in a file of its own under
/// `keys/<id>`, which this entry names. Keeping the two apart means the list of
/// hosts can be read, rewritten and exported without a key ever passing through
/// the same document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEntry {
    /// Stable, and also the file name of the material under `keys/`.
    pub id: String,
    /// What the user calls it.
    pub name: String,
}

/// The stored file's shape.
#[derive(Debug, Default, Serialize, Deserialize)]
struct HostsFile {
    version: u32,
    #[serde(default)]
    host: Vec<HostProfile>,
    /// The keychain. Names and ids only; see `KeyEntry`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    key: Vec<KeyEntry>,
}

/// The app's list of SSH profiles.
pub struct HostRepository {
    path: PathBuf,
    profiles: Vec<HostProfile>,
    keys: Vec<KeyEntry>,
}

impl HostRepository {
    /// Load the repository, or start an empty one if there is nothing stored.
    ///
    /// A missing file is not an error -- it is what a fresh install looks like --
    /// but a file that exists and cannot be understood is, because the
    /// alternative is silently starting empty and then overwriting it on the
    /// first save.
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(default_path())
    }

    pub fn load_from(path: PathBuf) -> anyhow::Result<Self> {
        let (profiles, keys) = match std::fs::read_to_string(&path) {
            Ok(text) => parse(&text).with_context(|| format!("reading {}", path.display()))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (vec![], vec![]),
            Err(err) => {
                return Err(err).with_context(|| format!("reading {}", path.display()));
            }
        };
        Ok(Self {
            path,
            profiles,
            keys,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn profiles(&self) -> &[HostProfile] {
        &self.profiles
    }

    pub fn get(&self, id: &str) -> Option<&HostProfile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    /// Add a profile. Its id must not already be present.
    pub fn add(&mut self, profile: HostProfile) -> anyhow::Result<()> {
        profile
            .validate()
            .map_err(|errors| anyhow!("{}", join_errors(&errors)))?;
        if self.get(&profile.id).is_some() {
            bail!("a host with id {} already exists", profile.id);
        }
        self.profiles.push(profile);
        self.save()
    }

    /// Replace a profile, bumping its generation.
    ///
    /// The bump is what stops the edited profile from colliding with the domain
    /// its previous version registered; see the module comment. It happens here
    /// rather than being left to the caller precisely because a caller that
    /// forgets produces a defect that reports nothing.
    pub fn update(&mut self, mut profile: HostProfile) -> anyhow::Result<()> {
        profile
            .validate()
            .map_err(|errors| anyhow!("{}", join_errors(&errors)))?;
        let existing = self
            .profiles
            .iter_mut()
            .find(|candidate| candidate.id == profile.id)
            .ok_or_else(|| anyhow!("no host with id {}", profile.id))?;

        if *existing == profile {
            // Nothing changed, so do not burn a generation and leave another
            // dead domain behind for the life of the process.
            return Ok(());
        }

        profile.generation = existing.generation.saturating_add(1);
        *existing = profile;
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> anyhow::Result<()> {
        let before = self.profiles.len();
        self.profiles.retain(|profile| profile.id != id);
        if self.profiles.len() == before {
            bail!("no host with id {id}");
        }
        self.save()
    }

    pub fn keys(&self) -> &[KeyEntry] {
        &self.keys
    }

    /// Where this repository's key material lives.
    ///
    /// Derived from the list's own location rather than from the global data
    /// directory, so that the keys belong to the list that names them. In
    /// production the two are the same directory; the difference is that a test
    /// pointed at a temporary file keeps its keys there too, instead of writing
    /// private keys into the developer's real data directory.
    fn key_dir(&self) -> PathBuf {
        match self.path.parent() {
            Some(parent) => parent.join("keys"),
            None => key_directory(),
        }
    }

    pub fn key(&self, id: &str) -> Option<&KeyEntry> {
        self.keys.iter().find(|key| key.id == id)
    }

    /// Store a private key under a name, and return the entry that now names it.
    ///
    /// The material is written first: if that fails there is nothing to point a
    /// dangling entry at, and the list is left exactly as it was.
    pub fn add_key(&mut self, name: &str, pem: &str) -> anyhow::Result<KeyEntry> {
        let name = name.trim();
        if name.is_empty() {
            bail!("the key needs a name");
        }

        let entry = KeyEntry {
            id: new_id(),
            name: name.to_string(),
        };
        import_key_into(&self.key_dir(), &entry.id, pem)?;

        self.keys.push(entry.clone());
        if let Err(err) = self.save() {
            // The list is the record of what exists. If it could not be written
            // then this key is not in it, and leaving the material behind would
            // leave an unreferenced private key on disk forever.
            let _ = std::fs::remove_file(self.key_dir().join(&entry.id));
            self.keys.retain(|key| key.id != entry.id);
            return Err(err);
        }
        Ok(entry)
    }

    /// Forget a key, and detach it from every host that used it.
    ///
    /// The profiles are updated in the same write, so there is no window in
    /// which a host names a key that is gone. Their generations are deliberately
    /// *not* bumped: the connection a stale domain would make is no longer one
    /// this profile can make anyway, and burning a generation per host on a
    /// single key deletion leaks a domain for each of them.
    pub fn remove_key(&mut self, id: &str) -> anyhow::Result<()> {
        let before = self.keys.len();
        self.keys.retain(|key| key.id != id);
        if self.keys.len() == before {
            bail!("no key with id {id}");
        }

        for profile in &mut self.profiles {
            if profile.key_id.as_deref() == Some(id) {
                profile.key_id = None;
            }
        }

        self.save()?;
        // After the list is written: a key file left behind by a failed unlink
        // is inert once nothing names it, whereas deleting it first and then
        // failing to save would leave hosts pointing at nothing.
        match std::fs::remove_file(self.key_dir().join(id)) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err).with_context(|| format!("removing the key material for {id}")),
        }
    }

    /// Where a profile's key material lives, if it has any.
    ///
    /// A `key_id` names something in the keychain. `key_file` is the older,
    /// blunter escape hatch: a path put there by hand. The keychain wins, since
    /// it is the one the UI can edit.
    pub fn key_path_for(&self, profile: &HostProfile) -> Option<PathBuf> {
        if let Some(id) = &profile.key_id {
            return self.key(id).map(|key| self.key_dir().join(&key.id));
        }
        profile.key_file.clone()
    }

    /// Delete the stored file and start over.
    ///
    /// The UI is the only editor this file has, so a way out of a broken list is
    /// part of the feature.
    pub fn reset(&mut self) -> anyhow::Result<()> {
        self.profiles.clear();
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err).with_context(|| format!("removing {}", self.path.display())),
        }
    }

    /// The text to hand to a share intent.
    ///
    /// Profiles only. A key path would name a file inside app-private storage
    /// that the recipient cannot read, and a password is never stored to begin
    /// with, so neither is included -- the export is something a user can read
    /// and retype, not a backup that restores secrets.
    pub fn export_document(&self) -> anyhow::Result<String> {
        let exported = HostsFile {
            version: FORMAT_VERSION,
            host: self
                .profiles
                .iter()
                .map(|profile| HostProfile {
                    key_file: None,
                    // Stripped for the same reason as `key_file`, though it is
                    // not itself a secret: it names a key inside this app's
                    // private storage, which the recipient does not have. A
                    // profile that arrived pointing at a key that cannot exist
                    // would fail to connect for a reason nothing explains.
                    key_id: None,
                    ..profile.clone()
                })
                .collect(),
            // The keychain is not exported at all. Without the material the
            // names are of no use to the recipient, and the material is exactly
            // what an export must never carry.
            key: vec![],
        };
        toml::to_string_pretty(&exported).context("serializing hosts for export")
    }

    fn save(&self) -> anyhow::Result<()> {
        let document = toml::to_string_pretty(&HostsFile {
            version: FORMAT_VERSION,
            host: self.profiles.clone(),
            key: self.keys.clone(),
        })
        .context("serializing hosts")?;
        write_atomically(&self.path, &document)
    }
}

fn join_errors(errors: &[ValidationError]) -> String {
    errors
        .iter()
        .map(|error| error.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

fn parse(text: &str) -> anyhow::Result<(Vec<HostProfile>, Vec<KeyEntry>)> {
    let file: HostsFile = toml::from_str(text).context("parsing the stored host list")?;
    if file.version > FORMAT_VERSION {
        bail!(
            "the stored host list is version {}, and this build understands up to {}",
            file.version,
            FORMAT_VERSION
        );
    }

    let mut seen = HashSet::new();
    for profile in &file.host {
        if !seen.insert(profile.id.clone()) {
            bail!("the stored host list has two hosts with id {}", profile.id);
        }
        if let Err(errors) = profile.validate() {
            bail!(
                "the stored host {} is invalid: {}",
                profile.display_name,
                join_errors(&errors)
            );
        }
    }

    let mut seen_keys = HashSet::new();
    for key in &file.key {
        if !seen_keys.insert(key.id.clone()) {
            bail!("the stored key list has two keys with id {}", key.id);
        }
    }

    // A host naming a key that is not in the list would fail to connect saying
    // only that the key file is missing. Refusing the whole file is the same
    // choice made for a host that will not validate: the UI is the only editor
    // this file has, and silently loading half of it invites overwriting the
    // rest.
    for profile in &file.host {
        if let Some(id) = &profile.key_id {
            if !seen_keys.contains(id) {
                bail!(
                    "the stored host {} names key {}, which is not in the key list",
                    profile.display_name,
                    id
                );
            }
        }
    }

    Ok((file.host, file.key))
}

/// Where the list is stored.
///
/// The data directory rather than the config directory: this file is written by
/// the app and never edited by hand, and the config directories are where
/// wezterm looks for things the user maintains. On Android both are inside
/// app-private storage regardless.
fn default_path() -> PathBuf {
    config::DATA_DIR.join("hosts.toml")
}

/// Where an imported private key is written.
pub fn key_directory() -> PathBuf {
    config::DATA_DIR.join("keys")
}

/// Write a private key that the user pasted in, and return its path.
///
/// The clipboard is a poor way to move a private key, and the alternative was no
/// key support at all on a device with no file picker and no reachable `HOME`.
/// So the transfer is accepted and then cleaned up after: the caller clears the
/// clipboard, and the file lands with `0600` in a `0700` directory.
pub fn import_key(id: &str, pem: &str) -> anyhow::Result<PathBuf> {
    import_key_into(&key_directory(), id, pem)
}

/// As `import_key`, into a named directory.
///
/// The directory is a parameter so the keychain can live beside the list that
/// names it; see `HostRepository::key_dir`.
fn import_key_into(dir: &Path, id: &str, pem: &str) -> anyhow::Result<PathBuf> {
    check_key_material(pem)?;
    let pem = pem.trim();

    create_private_dir(dir)?;
    let path = dir.join(id);

    // A trailing newline: some ssh implementations reject a key file without
    // one, and the clipboard commonly loses it.
    let mut document = pem.to_string();
    document.push('\n');
    write_atomically(&path, &document)?;
    Ok(path)
}

/// Reject something that is plainly not a private key.
///
/// Only the shape, not the cryptography: a key that parses here can still be
/// refused by the server, and telling the user that their paste is truncated is
/// worth more than proving it is a valid ed25519 key. The error is about the
/// text's form and never quotes it.
fn check_key_material(pem: &str) -> anyhow::Result<()> {
    let pem = pem.trim();
    if pem.is_empty() {
        bail!("the key is empty");
    }
    if !pem.starts_with("-----BEGIN") {
        bail!("that does not look like a private key: it must start with -----BEGIN");
    }
    if !pem.ends_with("-----") {
        bail!("the key looks truncated: it should end with a -----END line");
    }
    Ok(())
}

/// A directory only this app may enter.
fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("setting the mode on {}", dir.display()))?;
    }
    Ok(())
}

/// Replace a file's contents, or leave the old contents intact.
///
/// Write to a sibling, flush it to the device, then rename over the target. A
/// plain truncating write that is interrupted -- and on Android the process is
/// killed at the system's convenience -- leaves a half-written list, and the UI
/// is the only editor this file has, so there would be no way to repair it.
fn write_atomically(path: &Path, contents: &str) -> anyhow::Result<()> {
    use std::io::Write;

    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    create_private_dir(dir)?;

    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("{} has no file name", path.display()))?;
    let temp = dir.join(format!(".{}.new", name.to_string_lossy()));

    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp)
            .with_context(|| format!("creating {}", temp.display()))?;
        file.write_all(contents.as_bytes())
            .with_context(|| format!("writing {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("flushing {}", temp.display()))?;
    }

    std::fs::rename(&temp, path)
        .with_context(|| format!("renaming {} to {}", temp.display(), path.display()))?;

    // The rename itself has to reach the device too, or a crash can leave the
    // directory entry pointing at nothing.
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
    Ok(())
}

fn new_id() -> String {
    // 64 bits of randomness, which is ample for a list a person maintains by
    // hand and needs no coordination with anything else. A counter would be
    // shorter but would collide across an export and a reset.
    let mut id = String::with_capacity(16);
    for _ in 0..16 {
        id.push(char::from_digit(fastrand::u32(0..16), 16).unwrap_or('0'));
    }
    id
}

/// An SSH domain declared in `wezterm.lua`, for the sidebar to list.
///
/// Read-only: these belong to the config file, and the app has no business
/// rewriting Lua.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredDomain {
    pub name: String,
    pub remote_address: String,
    pub username: Option<String>,
    /// True for the flavour that needs a `wezterm` binary on the remote host.
    pub multiplexed: bool,
}

/// The SSH domains the config file declares.
pub fn configured_ssh_domains(config: &ConfigHandle) -> Vec<ConfiguredDomain> {
    config
        .ssh_domains()
        .into_iter()
        .map(|domain| ConfiguredDomain {
            name: domain.name,
            remote_address: domain.remote_address,
            username: domain.username,
            multiplexed: domain.multiplexing != SshMultiplexing::None,
        })
        .collect()
}

/// Register the profile's domain with the mux, and return it.
///
/// Idempotent for an unedited profile: the derived name is stable while the
/// generation is, so reconnecting to the same profile reuses the same domain and
/// its established session. An edited profile has a new name and therefore a new
/// domain, which is the point.
pub fn ensure_domain(
    profile: &HostProfile,
    key_path: Option<&Path>,
) -> anyhow::Result<Arc<dyn Domain>> {
    let mux = Mux::get();
    let name = profile.domain_name();

    if let Some(domain) = mux.get_domain_by_name(&name) {
        return Ok(domain);
    }

    let ssh_domain = profile.to_ssh_domain(key_path);
    // Two different domain types, chosen by the same flag the config file uses.
    // A multiplexed ssh domain is a *client* of a mux server reached over ssh,
    // not an ssh domain that happens to multiplex, which is why it cannot simply
    // be a RemoteSshDomain with a different setting.
    let domain: Arc<dyn Domain> = if profile.multiplexing {
        Arc::new(wezterm_client::domain::ClientDomain::new(
            wezterm_client::domain::ClientDomainConfig::Ssh(ssh_domain),
        ))
    } else {
        Arc::new(
            mux::ssh::RemoteSshDomain::with_ssh_domain(&ssh_domain)
                .with_context(|| format!("creating an ssh domain for {}", profile.display_name))?,
        )
    };
    mux.add_domain(&domain);
    Ok(domain)
}

/// The keys the host editor's answers arrive under.
pub mod field {
    pub const DISPLAY_NAME: &str = "display_name";
    pub const HOST: &str = "host";
    pub const PORT: &str = "port";
    pub const USERNAME: &str = "username";
    pub const PRIVATE_KEY: &str = "private_key";
    pub const KEY_ID: &str = "key_id";
    pub const KEY_NAME: &str = "key_name";
    pub const MULTIPLEXING: &str = "multiplexing";
}

/// The `key_id` a profile with no key answers with.
///
/// The empty string rather than a missing field: a picker always answers with
/// whichever row is selected, and "no key" has to be one of the rows.
const NO_KEY: &str = "";

/// The add/edit form, with any validation messages attached to their fields.
///
/// The errors travel in the spec rather than being reported separately, so
/// redisplaying a rejected form keeps whatever the user typed. A dialog that
/// cleared itself to report one typo would make them retype the rest.
fn editor_spec(
    draft: &HostProfile,
    errors: &[ValidationError],
    is_new: bool,
    keys: &[KeyEntry],
) -> DialogSpec {
    let error_for = |field: Field| {
        errors
            .iter()
            .find(|error| error.field == field)
            .map(|error| error.message.clone())
    };

    // "No key" first, so that it is what a profile with nothing chosen shows,
    // and so the list reads as a choice rather than as a requirement.
    let mut options = vec![ChoiceOption::new(NO_KEY, "No key (password login)")];
    options.extend(keys.iter().map(|key| ChoiceOption::new(&key.id, &key.name)));

    DialogSpec::new(if is_new { "Add host" } else { "Edit host" })
        .submit_label("Save")
        .field(
            DialogField::new(
                field::DISPLAY_NAME,
                "Name",
                FieldKind::Text,
                &draft.display_name,
            )
            .hint("What the sidebar shows")
            .error(error_for(Field::DisplayName)),
        )
        .field(
            DialogField::new(field::HOST, "Host", FieldKind::Text, &draft.host)
                .hint("host name or IP address")
                .error(error_for(Field::Host)),
        )
        .field(
            DialogField::new(
                field::PORT,
                "Port",
                FieldKind::Number,
                &draft.port.to_string(),
            )
            .error(error_for(Field::Port)),
        )
        .field(
            DialogField::new(field::USERNAME, "User", FieldKind::Text, &draft.username)
                .error(error_for(Field::Username)),
        )
        .field(DialogField::choice(
            field::KEY_ID,
            "Key",
            draft.key_id.as_deref().unwrap_or(NO_KEY),
            options,
        ))
        .field(
            DialogField::toggle(
                field::MULTIPLEXING,
                "Keep session running",
                draft.multiplexing,
            )
            // Named for what it does rather than for how: "multiplexing" is the
            // mechanism, and the cost is the part a user has to agree to.
            .hint(
                "Panes stay alive on the server when you close the app, and \
                 reconnecting returns to them. Needs wezterm-mux-server installed \
                 on that host, matching this app's version.",
            ),
        )
}

/// Apply the editor's answers to a draft.
///
/// A port that will not parse is left as `0` rather than silently reverting to
/// the previous value or to 22: validation then rejects it and says so, which is
/// the only outcome that tells the user what happened.
fn apply_editor_values(draft: &mut HostProfile, values: &DialogValues) {
    draft.display_name = values.get(field::DISPLAY_NAME).trim().to_string();
    draft.host = values.get(field::HOST).trim().to_string();
    draft.port = values.parse_u16(field::PORT).unwrap_or(0);
    draft.username = values.get(field::USERNAME).trim().to_string();
    draft.multiplexing = values.parse_bool(field::MULTIPLEXING);

    let key_id = values.get(field::KEY_ID).trim();
    draft.key_id = if key_id == NO_KEY {
        None
    } else {
        Some(key_id.to_string())
    };
    // Choosing from the keychain retires the hand-placed path, so that the two
    // cannot both be set and disagree about which key this host uses.
    if draft.key_id.is_some() {
        draft.key_file = None;
    }
}

/// Show the add/edit form until it validates, or the user gives up.
///
/// `None` means cancelled. The returned profile has not been stored: the caller
/// decides between `HostRepository::add` and `update`, which is also what decides
/// whether a generation is spent.
pub async fn edit_interactively(
    existing: Option<&HostProfile>,
    keys: &[KeyEntry],
) -> anyhow::Result<Option<HostProfile>> {
    let is_new = existing.is_none();
    let mut draft = match existing {
        Some(profile) => profile.clone(),
        None => HostProfile::new("", "", DEFAULT_PORT, ""),
    };
    let mut errors: Vec<ValidationError> = vec![];

    loop {
        let spec = editor_spec(&draft, &errors, is_new, keys);
        let Some(values) = crate::dialog::present(&spec).await? else {
            return Ok(None);
        };

        apply_editor_values(&mut draft, &values);
        match draft.validate() {
            Ok(()) => return Ok(Some(draft)),
            // Round again with the same values and the messages beside the
            // fields that earned them.
            Err(found) => errors = found,
        }
    }
}

/// A key the user has just pasted, and what they want it called.
///
/// The material is carried no further than the caller that stores it; see
/// `HostRepository::add_key`.
pub struct ImportedKey {
    pub name: String,
    pub pem: String,
}

/// Ask for a private key and a name for it.
///
/// `None` means cancelled. The dialog states plainly that the key passes through
/// the clipboard, and clears the clipboard on submit; see `import_key` for why
/// that route is accepted at all.
///
/// This only collects: it does not write, because the keychain that decides the
/// key's id lives behind the repository and the dialog cannot reach it from the
/// task it runs on.
pub async fn import_key_interactively() -> anyhow::Result<Option<ImportedKey>> {
    let mut error: Option<String> = None;
    let mut name = String::new();

    loop {
        let spec = DialogSpec::new("Add key")
            .message(
                "Paste an OpenSSH private key. It is stored inside this app's \
                 private storage and never leaves the device.\n\n\
                 Note that pasting puts the key on the system clipboard, where \
                 Android keeps a history of recent clips and a cloud-syncing \
                 keyboard may see it. The clipboard is cleared when you save.",
            )
            .submit_label("Import")
            .clear_clipboard_on_submit(true)
            .field(
                DialogField::new(field::KEY_NAME, "Name", FieldKind::Text, &name)
                    .hint("What to call this key"),
            )
            .field(
                DialogField::new(field::PRIVATE_KEY, "Key", FieldKind::SecretMultiline, "")
                    .hint("-----BEGIN OPENSSH PRIVATE KEY-----")
                    .error(error.take()),
            );

        let Some(mut values) = crate::dialog::present(&spec).await? else {
            return Ok(None);
        };

        name = values.get(field::KEY_NAME).trim().to_string();
        // Taken rather than read, so the key does not sit in the reply map for
        // longer than the caller that stores it needs it.
        let pem = values.take(field::PRIVATE_KEY);

        // Checked here so that a bad paste is reported against the field that
        // holds it, with the name the user already typed still in place. The
        // name is deliberately kept across the round trip and the key is not:
        // retyping a name is an annoyance, and a key that survived a rejected
        // dialog would be a key sitting in a view for longer than it must.
        if let Err(err) = check_key_material(&pem) {
            error = Some(format!("{err:#}"));
            continue;
        }
        if name.is_empty() {
            error = Some("the key needs a name".to_string());
            continue;
        }

        return Ok(Some(ImportedKey { name, pem }));
    }
}

/// Routes `wezterm-ssh`'s interactive prompts to the native dialogs.
///
/// See `mux::sshprompt` for why they do not stay in the pane. Every method here
/// is called from the ssh event loop's own thread, which already blocks on each
/// prompt, so blocking on a dialog is what the caller expects.
struct NativeSshPrompter;

impl mux::sshprompt::SshPrompter for NativeSshPrompter {
    fn verify_host(&self, message: &str) -> anyhow::Result<bool> {
        let spec = crate::dialog::host_verify_spec(message);
        smol::block_on(crate::dialog::confirm(&spec))
    }

    fn answer_prompt(&self, prompt: &str, echo: bool) -> anyhow::Result<String> {
        let spec = crate::dialog::credential_spec(prompt, echo);
        match smol::block_on(crate::dialog::present(&spec))? {
            Some(mut values) => Ok(values.take(crate::dialog::CREDENTIAL_KEY)),
            // A deliberate dismissal, which must fail the connection rather than
            // fall through to a second prompt in the pane.
            None => Err(mux::sshprompt::Cancelled.into()),
        }
    }

    fn host_verification_failed(&self, message: &str) {
        let spec = crate::dialog::host_verification_failed_spec(message);
        // The answer is discarded: there is nothing to decide. Errors are logged
        // rather than propagated because the pane shows the same warning anyway.
        if let Err(err) = smol::block_on(crate::dialog::confirm(&spec)) {
            log::warn!("could not show the host key warning: {err:#}");
        }
    }
}

/// Hand the exported host list to the platform's share mechanism.
///
/// The stored file lives in app-private storage and `run-as` is refused for a
/// package that is not debuggable, so a share intent is the only route anything
/// has off the device. It needs no storage permission and no file picker.
pub fn share_text(document: &str) -> anyhow::Result<()> {
    window::dialog::share_text("wezterm hosts", document)
}

/// Send ssh prompts to native dialogs, where there are any.
pub fn install_ssh_prompter() {
    if !crate::dialog::available() {
        return;
    }
    mux::sshprompt::set_prompter(Arc::new(NativeSshPrompter));
}

#[cfg(test)]
mod test {
    use super::*;

    fn valid() -> HostProfile {
        HostProfile::new("dev box", "dev.example.com", 22, "ysbf")
    }

    #[test]
    fn a_valid_profile_validates() {
        assert_eq!(valid().validate(), Ok(()));
    }

    #[test]
    fn a_profile_reports_every_problem_at_once() {
        // All of them, because the editor shows the messages beside the fields
        // and fixing one per round trip through a dialog is miserable.
        let profile = HostProfile {
            display_name: "  ".to_string(),
            host: "not a host".to_string(),
            port: 0,
            username: "".to_string(),
            ..valid()
        };
        let errors = profile.validate().unwrap_err();
        let fields: Vec<Field> = errors.iter().map(|error| error.field).collect();
        assert_eq!(
            fields,
            vec![
                Field::DisplayName,
                Field::Host,
                Field::Port,
                Field::Username
            ]
        );
    }

    #[test]
    fn hosts_may_be_names_or_addresses() {
        for host in [
            "example.com",
            "dev",
            "a-b.example.co.uk",
            "192.168.1.10",
            "::1",
            "[2001:db8::1]",
            "host_with_underscore",
        ] {
            assert!(validate_host(host).is_ok(), "{} should be accepted", host);
        }
    }

    #[test]
    fn a_host_with_a_port_is_rejected_rather_than_timing_out() {
        // The port has its own field. Two places that disagree about which wins
        // is worse than a message.
        assert!(validate_host("example.com:2222").is_err());
        // And so is anything ssh could not resolve, which would otherwise fail
        // as a twenty second timeout that reads as an unreachable server.
        for host in [
            "",
            " ",
            "has space",
            "user@example.com",
            "a/b",
            "-lead.com",
            "trail-.com",
            "a..b",
        ] {
            assert!(
                validate_host(host).is_err(),
                "{:?} should be rejected",
                host
            );
        }
    }

    #[test]
    fn usernames_reject_what_ssh_would_reparse() {
        assert!(validate_username("ysbf").is_ok());
        for name in ["", "two words", "user@host", "user:pass"] {
            assert!(
                validate_username(name).is_err(),
                "{:?} should be rejected",
                name
            );
        }
    }

    #[test]
    fn a_profile_maps_to_a_plain_ssh_domain() {
        let domain = valid().to_ssh_domain(None);
        // The whole point: the multiplexing flavour would run `wezterm cli
        // proxy` on the far end and fail against an ordinary sshd. It is also
        // the default for SshDomain, so this is a thing to get wrong by
        // omission.
        assert_eq!(domain.multiplexing, SshMultiplexing::None);
        // The port rides in remote_address because that is the only channel
        // ssh_domain_to_ssh_config reads it from.
        assert_eq!(domain.remote_address, "dev.example.com:22");
        assert_eq!(domain.username.as_deref(), Some("ysbf"));
    }

    #[test]
    fn a_key_file_turns_off_agent_and_other_keys() {
        let mut profile = valid();
        profile.key_file = Some(PathBuf::from("/data/keys/abc"));
        let domain = profile.to_ssh_domain(Some(Path::new("/data/keys/abc")));
        assert_eq!(
            domain.ssh_option.get("identityfile").map(|s| s.as_str()),
            Some("/data/keys/abc")
        );
        // Offering unrelated keys to a server is how an account gets locked out.
        assert_eq!(
            domain.ssh_option.get("identitiesonly").map(|s| s.as_str()),
            Some("yes")
        );
    }

    #[test]
    fn multiplexing_is_off_unless_asked_for() {
        // SshMultiplexing's own default is WezTerm, which fails against a plain
        // sshd. A profile that says nothing must not inherit that.
        assert_eq!(
            valid().to_ssh_domain(None).multiplexing,
            SshMultiplexing::None
        );

        let mut profile = valid();
        profile.multiplexing = true;
        assert_eq!(
            profile.to_ssh_domain(None).multiplexing,
            SshMultiplexing::WezTerm
        );
    }

    #[test]
    fn a_stored_profile_that_says_nothing_about_keys_or_multiplexing_still_loads() {
        // The fields were added after the format was in use, so a file written
        // before them has to keep working rather than failing to parse.
        let text = format!(
            "version = {FORMAT_VERSION}\n\
             [[host]]\n\
             id = \"abc\"\n\
             display_name = \"dev\"\n\
             host = \"dev.example.com\"\n\
             username = \"ysbf\"\n"
        );
        let (hosts, keys) = parse(&text).unwrap();
        assert_eq!(hosts.len(), 1);
        assert!(!hosts[0].multiplexing);
        assert_eq!(hosts[0].key_id, None);
        assert_eq!(hosts[0].port, DEFAULT_PORT);
        assert!(keys.is_empty());
    }

    #[test]
    fn a_host_naming_a_key_that_is_not_there_refuses_to_load() {
        // Loading it would connect without the key and report only that some
        // file was missing, which says nothing about the list being wrong.
        let text = format!(
            "version = {FORMAT_VERSION}\n\
             [[host]]\n\
             id = \"abc\"\n\
             display_name = \"dev\"\n\
             host = \"dev.example.com\"\n\
             username = \"ysbf\"\n\
             key_id = \"ghost\"\n"
        );
        assert!(parse(&text).is_err());
    }

    #[test]
    fn choosing_a_keychain_key_retires_the_hand_placed_path() {
        // Both set would leave two answers to "which key does this host use".
        let mut draft = valid();
        draft.key_file = Some(PathBuf::from("/data/keys/by-hand"));
        apply_editor_values(
            &mut draft,
            &DialogValues::from_pairs(&[
                ("display_name", "dev"),
                ("host", "dev.example.com"),
                ("port", "22"),
                ("username", "ysbf"),
                ("key_id", "chosen"),
                ("multiplexing", "true"),
            ]),
        );
        assert_eq!(draft.key_id.as_deref(), Some("chosen"));
        assert_eq!(draft.key_file, None);
        assert!(draft.multiplexing);
    }

    #[test]
    fn choosing_no_key_clears_the_choice_rather_than_storing_an_empty_id() {
        let mut draft = valid();
        draft.key_id = Some("chosen".to_string());
        apply_editor_values(
            &mut draft,
            &DialogValues::from_pairs(&[
                ("display_name", "dev"),
                ("host", "dev.example.com"),
                ("port", "22"),
                ("username", "ysbf"),
                ("key_id", ""),
                ("multiplexing", "false"),
            ]),
        );
        assert_eq!(draft.key_id, None);
        assert!(!draft.multiplexing);
    }

    #[test]
    fn deleting_a_key_detaches_it_from_the_hosts_that_used_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut repo = HostRepository::load_from(dir.path().join("hosts.toml")).unwrap();

        let key = repo
            .add_key(
                "laptop",
                "-----BEGIN OPENSSH PRIVATE KEY-----\nx\n-----END-----",
            )
            .unwrap();
        let mut profile = valid();
        profile.key_id = Some(key.id.clone());
        repo.add(profile.clone()).unwrap();

        repo.remove_key(&key.id).unwrap();

        // Not merely absent from the list: the host must not be left naming it,
        // or the file it just wrote would refuse to load next time.
        assert!(repo.keys().is_empty());
        assert_eq!(repo.get(&profile.id).unwrap().key_id, None);
        let reloaded = HostRepository::load_from(dir.path().join("hosts.toml")).unwrap();
        assert_eq!(reloaded.get(&profile.id).unwrap().key_id, None);
    }

    #[test]
    fn a_key_serves_more_than_one_host() {
        // The point of a keychain over a key per profile.
        let dir = tempfile::tempdir().unwrap();
        let mut repo = HostRepository::load_from(dir.path().join("hosts.toml")).unwrap();

        let key = repo
            .add_key(
                "shared",
                "-----BEGIN OPENSSH PRIVATE KEY-----\nx\n-----END-----",
            )
            .unwrap();
        for name in ["one", "two"] {
            let mut profile = HostProfile::new(name, "dev.example.com", 22, "ysbf");
            profile.key_id = Some(key.id.clone());
            repo.add(profile).unwrap();
        }

        let paths: Vec<_> = repo
            .profiles()
            .iter()
            .map(|profile| repo.key_path_for(profile))
            .collect();
        assert_eq!(paths[0], paths[1]);
        assert!(paths[0].is_some());
    }

    #[test]
    fn an_export_carries_neither_key_material_nor_a_reference_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut repo = HostRepository::load_from(dir.path().join("hosts.toml")).unwrap();

        let key = repo
            .add_key(
                "laptop",
                "-----BEGIN OPENSSH PRIVATE KEY-----\nsecret\n-----END-----",
            )
            .unwrap();
        let mut profile = valid();
        profile.key_id = Some(key.id.clone());
        profile.key_file = Some(PathBuf::from("/data/keys/by-hand"));
        repo.add(profile).unwrap();

        let exported = repo.export_document().unwrap();
        assert!(!exported.contains("secret"));
        assert!(!exported.contains("BEGIN OPENSSH"));
        assert!(!exported.contains("by-hand"));
        assert!(!exported.contains(&key.id));
        assert!(!exported.contains("laptop"));
        // The profile itself still travels.
        assert!(exported.contains("dev.example.com"));
    }

    #[test]
    fn a_truncated_key_is_refused_before_it_is_stored() {
        assert!(check_key_material("").is_err());
        assert!(check_key_material("ssh-rsa AAAA").is_err());
        // A paste that lost its tail is the common clipboard failure.
        assert!(check_key_material("-----BEGIN OPENSSH PRIVATE KEY-----\nabc").is_err());
        assert!(
            check_key_material("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END-----").is_ok()
        );
    }

    #[test]
    fn an_edit_yields_a_domain_name_that_was_never_registered() {
        let dir = tempfile::tempdir().unwrap();
        let mut repo = HostRepository::load_from(dir.path().join("hosts.toml")).unwrap();

        let profile = valid();
        let id = profile.id.clone();
        repo.add(profile).unwrap();
        let first = repo.get(&id).unwrap().domain_name();

        let mut edited = repo.get(&id).unwrap().clone();
        edited.port = 2222;
        repo.update(edited).unwrap();
        let second = repo.get(&id).unwrap().domain_name();

        // Mux registration is first-write-wins and silent, so reusing the name
        // would make the reconnect use the *old* port with nothing reported.
        assert_ne!(first, second);
        assert_eq!(repo.get(&id).unwrap().generation, 1);
    }

    #[test]
    fn an_edit_that_changes_nothing_does_not_burn_a_generation() {
        // Every generation leaves a dead domain behind for the life of the
        // process, so they are not spent for nothing.
        let dir = tempfile::tempdir().unwrap();
        let mut repo = HostRepository::load_from(dir.path().join("hosts.toml")).unwrap();
        let profile = valid();
        let id = profile.id.clone();
        repo.add(profile.clone()).unwrap();

        repo.update(profile).unwrap();
        assert_eq!(repo.get(&id).unwrap().generation, 0);
    }

    #[test]
    fn profiles_round_trip_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hosts.toml");

        let mut repo = HostRepository::load_from(path.clone()).unwrap();
        repo.add(valid()).unwrap();
        repo.add(HostProfile::new("prod", "10.0.0.1", 2222, "root"))
            .unwrap();

        let reloaded = HostRepository::load_from(path).unwrap();
        assert_eq!(reloaded.profiles(), repo.profiles());
    }

    #[test]
    fn a_missing_file_is_an_empty_repository_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let repo = HostRepository::load_from(dir.path().join("nope.toml")).unwrap();
        assert!(repo.profiles().is_empty());
    }

    #[test]
    fn an_unreadable_file_refuses_to_load() {
        // Rather than starting empty and overwriting the user's only copy on the
        // first save.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hosts.toml");
        std::fs::write(&path, "this is not toml {{{").unwrap();
        assert!(HostRepository::load_from(path).is_err());
    }

    #[test]
    fn a_newer_format_version_refuses_to_load() {
        let text = format!("version = {}\n", FORMAT_VERSION + 1);
        assert!(parse(&text).is_err());
        // But the current one is fine, including with no hosts and no keys.
        assert_eq!(
            parse(&format!("version = {FORMAT_VERSION}\n")).unwrap(),
            (vec![], vec![])
        );
    }

    #[test]
    fn duplicate_ids_refuse_to_load() {
        let text = format!(
            r#"
version = {FORMAT_VERSION}

[[host]]
id = "same"
display_name = "one"
host = "a.example.com"
port = 22
username = "u"

[[host]]
id = "same"
display_name = "two"
host = "b.example.com"
port = 22
username = "u"
"#
        );
        assert!(parse(&text).is_err());
    }

    #[test]
    fn an_invalid_profile_is_rejected_on_the_way_in_and_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut repo = HostRepository::load_from(dir.path().join("hosts.toml")).unwrap();
        let bad = HostProfile::new("", "", 0, "");
        assert!(repo.add(bad).is_err());
        assert!(repo.profiles().is_empty());

        let text = format!(
            r#"
version = {FORMAT_VERSION}

[[host]]
id = "abc"
display_name = "bad"
host = "has a space"
port = 22
username = "u"
"#
        );
        assert!(parse(&text).is_err());
    }

    #[test]
    fn export_excludes_the_key_path() {
        // A path inside app-private storage means nothing to whoever receives
        // the export, and the export must carry no secrets.
        let dir = tempfile::tempdir().unwrap();
        let mut repo = HostRepository::load_from(dir.path().join("hosts.toml")).unwrap();
        let mut profile = valid();
        profile.key_file = Some(PathBuf::from("/data/data/com.example/keys/abc"));
        repo.add(profile).unwrap();

        let document = repo.export_document().unwrap();
        assert!(!document.contains("key_file"));
        assert!(!document.contains("/data/data"));
        assert!(document.contains("dev.example.com"));
    }

    #[test]
    fn reset_empties_the_repository_and_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hosts.toml");
        let mut repo = HostRepository::load_from(path.clone()).unwrap();
        repo.add(valid()).unwrap();
        assert!(path.exists());

        repo.reset().unwrap();
        assert!(repo.profiles().is_empty());
        assert!(!path.exists());
        // And resetting again is not an error.
        repo.reset().unwrap();
    }

    #[test]
    fn remove_reports_an_unknown_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut repo = HostRepository::load_from(dir.path().join("hosts.toml")).unwrap();
        assert!(repo.remove("nope").is_err());
    }

    #[test]
    fn a_failed_write_leaves_the_old_contents() {
        // The temp-and-rename is what guarantees this; a truncating write that
        // the system interrupts would leave half a list and no way to repair it,
        // since the UI is the only editor this file has.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hosts.toml");
        write_atomically(&path, "version = 1\n").unwrap();
        write_atomically(&path, "version = 1\n# second\n").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# second"));
        // No stray temporary file left behind.
        let strays: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".new"))
            .collect();
        assert!(strays.is_empty());
    }

    #[test]
    fn ids_are_distinct() {
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            assert!(seen.insert(new_id()));
        }
    }

    #[test]
    fn a_pasted_key_must_look_like_one() {
        assert!(import_key("id", "").is_err());
        assert!(import_key("id", "ssh-rsa AAAA...").is_err());
    }

    #[test]
    fn the_editor_offers_the_profile_it_is_editing() {
        let profile = valid();
        let spec = editor_spec(&profile, &[], false, &[]);
        assert_eq!(spec.title, "Edit host");
        let value_of = |key: &str| {
            spec.fields
                .iter()
                .find(|field| field.key == key)
                .map(|field| field.value.clone())
                .unwrap()
        };
        assert_eq!(value_of(field::DISPLAY_NAME), "dev box");
        assert_eq!(value_of(field::HOST), "dev.example.com");
        assert_eq!(value_of(field::PORT), "22");
        assert_eq!(value_of(field::USERNAME), "ysbf");
        // The port gets a numeric keyboard rather than a full one.
        let port = spec.fields.iter().find(|f| f.key == field::PORT).unwrap();
        assert_eq!(port.kind, FieldKind::Number);
    }

    #[test]
    fn a_rejected_form_keeps_the_typed_values_and_gains_messages() {
        // This is the whole reason errors travel in the spec: a dialog that
        // cleared itself to report one typo would make the user retype the rest.
        let mut draft = valid();
        draft.host = "has a space".to_string();
        let errors = draft.validate().unwrap_err();

        let spec = editor_spec(&draft, &errors, false, &[]);
        let host = spec.fields.iter().find(|f| f.key == field::HOST).unwrap();
        assert_eq!(host.value, "has a space");
        assert!(host.error.is_some());
        // And the fields that were fine carry no message.
        let name = spec
            .fields
            .iter()
            .find(|f| f.key == field::DISPLAY_NAME)
            .unwrap();
        assert!(name.error.is_none());
    }

    #[test]
    fn a_new_host_starts_empty_on_the_default_port() {
        let draft = HostProfile::new("", "", DEFAULT_PORT, "");
        let spec = editor_spec(&draft, &[], true, &[]);
        assert_eq!(spec.title, "Add host");
        let port = spec.fields.iter().find(|f| f.key == field::PORT).unwrap();
        assert_eq!(port.value, "22");
    }

    #[test]
    fn the_editors_answers_are_trimmed_onto_the_draft() {
        let mut draft = valid();
        let before = draft.clone();
        apply_editor_values(
            &mut draft,
            &DialogValues::from_pairs(&[
                (field::DISPLAY_NAME, "  prod  "),
                (field::HOST, " 10.0.0.1 "),
                (field::PORT, "2222"),
                (field::USERNAME, " root "),
            ]),
        );
        assert_eq!(draft.display_name, "prod");
        assert_eq!(draft.host, "10.0.0.1");
        assert_eq!(draft.port, 2222);
        assert_eq!(draft.username, "root");
        // The identity and generation are the draft's, not the form's: they are
        // what stops an edited profile colliding with the domain its previous
        // version registered, so the form must not be able to touch them.
        assert_eq!(draft.id, before.id);
        assert_eq!(draft.generation, before.generation);
    }

    #[test]
    fn a_port_that_will_not_parse_is_rejected_rather_than_guessed() {
        // Silently reverting to 22, or to the previous value, would connect
        // somewhere the user did not ask for and say nothing.
        let mut draft = valid();
        apply_editor_values(
            &mut draft,
            &DialogValues::from_pairs(&[
                (field::DISPLAY_NAME, "dev"),
                (field::HOST, "dev.example.com"),
                (field::PORT, "not a port"),
                (field::USERNAME, "ysbf"),
            ]),
        );
        assert_eq!(draft.port, 0);
        let errors = draft.validate().unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, Field::Port);
    }
}
