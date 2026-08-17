//! Somewhere other than the pane to ask an SSH question.
//!
//! Connecting over ssh is not one request that succeeds or fails: two
//! interactive prompts sit in the middle of it. `wezterm-ssh` raises
//! `SessionEvent::HostVerify` and **blocks the connection** until it is
//! answered, and `SessionEvent::Authenticate` for a password or a key
//! passphrase.
//!
//! Both are normally answered inline in the pane being spawned, with a line
//! editor drawn into the terminal it is about to become. That is the right
//! answer on a desktop and the wrong one on a phone:
//!
//! * a password typed into the pane goes through the soft keyboard, so a
//!   cloud-syncing IME sees it and may learn it, whereas a native password field
//!   can say it is not for autofill and not for suggestion;
//! * a host key mismatch presented as terminal text can be scrolled past, and it
//!   is the one prompt that must not be dismissed by accident;
//! * the prompt has to be reachable while the host sidebar is open, and the
//!   sidebar is drawn over the panes.
//!
//! So a front end may register a prompter and take those questions somewhere
//! else. Registering one is optional and there is exactly one: it belongs to the
//! process, like the front end itself. With none registered, nothing changes.
//!
//! Every method is blocking. The ssh event loop runs on a thread of its own and
//! already blocks on each prompt, so an implementation is free to wait on a
//! dialog. A method that returns `Err` falls back to the in-pane prompt rather
//! than failing the connection: a front end whose dialog could not be shown
//! should not make the host unreachable.

use std::sync::{Arc, Mutex};

/// The user dismissed a prompt.
///
/// Distinguished from any other error because the two mean opposite things: a
/// prompter that *could not ask* falls back to the in-pane prompt, whereas a user
/// who said no must not then be asked the same question a second way.
#[derive(Debug, thiserror::Error)]
#[error("the prompt was cancelled")]
pub struct Cancelled;

pub trait SshPrompter: Send + Sync {
    /// Ask whether to trust a host key that is not yet in `known_hosts`.
    ///
    /// Every first connection to a new host reaches this, so with a sidebar full
    /// of hosts it is the common path and not an edge case.
    fn verify_host(&self, message: &str) -> anyhow::Result<bool>;

    /// Ask for one authentication answer. `echo` is false for a password or a
    /// passphrase, which must be masked.
    ///
    /// Return [`Cancelled`] when the user dismissed the prompt; any other error
    /// means "I could not ask", and the question goes to the pane instead.
    fn answer_prompt(&self, prompt: &str, echo: bool) -> anyhow::Result<String>;

    /// Report that a host key did not match the one on record.
    ///
    /// Not a question: there is no "yes" to offer. It must be presented so that
    /// it cannot be dismissed by accident.
    fn host_verification_failed(&self, message: &str);
}

static PROMPTER: Mutex<Option<Arc<dyn SshPrompter>>> = Mutex::new(None);

/// Route ssh prompts to `prompter` instead of into the pane.
pub fn set_prompter(prompter: Arc<dyn SshPrompter>) {
    *PROMPTER.lock().unwrap() = Some(prompter);
}

/// The registered prompter, if a front end installed one.
pub fn prompter() -> Option<Arc<dyn SshPrompter>> {
    PROMPTER.lock().unwrap().clone()
}
