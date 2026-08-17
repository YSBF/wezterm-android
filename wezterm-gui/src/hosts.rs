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
//! A sidebar profile is an ordinary SSH login, so it must be the first: the
//! multiplexing flavour computes a `wezterm` path and executes it on the far
//! end, and fails against a plain server. `SshMultiplexing` defaults to
//! `WezTerm`, so this is a thing to get wrong by omission.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_file: Option<PathBuf>,
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
    /// `multiplexing: None` is the point of this function. See the module
    /// comment for what the other flavour would do.
    pub fn to_ssh_domain(&self) -> SshDomain {
        SshDomain {
            name: self.domain_name(),
            // `ssh_domain_to_ssh_config` splits a trailing `:port` off this, and
            // that is the only channel it has for the port.
            remote_address: format!("{}:{}", self.host, self.port),
            username: Some(self.username.clone()),
            multiplexing: SshMultiplexing::None,
            ssh_option: self.ssh_options(),
            ..SshDomain::default()
        }
    }

    fn ssh_options(&self) -> std::collections::HashMap<String, String> {
        let mut options = std::collections::HashMap::new();
        if let Some(key) = &self.key_file {
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

/// The stored file's shape.
#[derive(Debug, Default, Serialize, Deserialize)]
struct HostsFile {
    version: u32,
    #[serde(default)]
    host: Vec<HostProfile>,
}

/// The app's list of SSH profiles.
pub struct HostRepository {
    path: PathBuf,
    profiles: Vec<HostProfile>,
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
        let profiles = match std::fs::read_to_string(&path) {
            Ok(text) => parse(&text).with_context(|| format!("reading {}", path.display()))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(err) => {
                return Err(err).with_context(|| format!("reading {}", path.display()));
            }
        };
        Ok(Self { path, profiles })
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
                    ..profile.clone()
                })
                .collect(),
        };
        toml::to_string_pretty(&exported).context("serializing hosts for export")
    }

    fn save(&self) -> anyhow::Result<()> {
        let document = toml::to_string_pretty(&HostsFile {
            version: FORMAT_VERSION,
            host: self.profiles.clone(),
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

fn parse(text: &str) -> anyhow::Result<Vec<HostProfile>> {
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

    Ok(file.host)
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
    let pem = pem.trim();
    if pem.is_empty() {
        bail!("the key is empty");
    }
    if !pem.starts_with("-----BEGIN") {
        bail!("that does not look like a private key: it must start with -----BEGIN");
    }

    let dir = key_directory();
    create_private_dir(&dir)?;
    let path = dir.join(id);

    // A trailing newline: some ssh implementations reject a key file without
    // one, and the clipboard commonly loses it.
    let mut document = pem.to_string();
    document.push('\n');
    write_atomically(&path, &document)?;
    Ok(path)
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
pub fn ensure_domain(profile: &HostProfile) -> anyhow::Result<Arc<dyn Domain>> {
    let mux = Mux::get();
    let name = profile.domain_name();

    if let Some(domain) = mux.get_domain_by_name(&name) {
        return Ok(domain);
    }

    let ssh_domain = profile.to_ssh_domain();
    let domain: Arc<dyn Domain> = Arc::new(
        mux::ssh::RemoteSshDomain::with_ssh_domain(&ssh_domain)
            .with_context(|| format!("creating an ssh domain for {}", profile.display_name))?,
    );
    mux.add_domain(&domain);
    Ok(domain)
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
        let domain = valid().to_ssh_domain();
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
        let domain = profile.to_ssh_domain();
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
        // But the current one is fine, including with no hosts in it.
        assert_eq!(
            parse(&format!("version = {FORMAT_VERSION}\n")).unwrap(),
            vec![]
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
}
