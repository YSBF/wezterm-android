//! The schema for the native dialogs, and the specs the app asks for.
//!
//! `window::dialog` is the transport: it hands a JSON document to the platform
//! and returns a JSON answer or a cancellation, without looking at either. This
//! module owns what those documents mean, and is the only place that has to
//! agree with `WezTermDialogs.kt`.
//!
//! There are two kinds of dialog and one degenerate case:
//!
//! * a **form** -- the host editor, with name, host, port and user fields, and a
//!   private key to paste;
//! * a **credential prompt** -- a single masked field for a password or a key
//!   passphrase;
//! * a **notice** -- no fields at all, used for the host key questions, where the
//!   answer is which button was pressed.
//!
//! Validation errors are carried *in* the spec rather than reported separately,
//! so redisplaying a form after a rejected save means building it again from the
//! values the user typed with the messages attached. A dialog that cleared itself
//! to report a typo would make them retype everything.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use window::dialog::{request_dialog, DialogOutcome};

/// What sort of editor a field gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Number,
    /// Masked, kept away from autofill and from the IME's suggestions.
    Password,
    /// Several lines, unmasked but kept away from autofill and suggestions.
    ///
    /// Unmasked because a private key is pasted rather than typed and the user
    /// needs to see that the paste landed.
    SecretMultiline,
}

#[derive(Debug, Clone, Serialize)]
pub struct DialogField {
    /// How the answer is keyed in the reply.
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    /// What the field starts out containing.
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// A validation message to show beside the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DialogField {
    pub fn new(key: &str, label: &str, kind: FieldKind, value: &str) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            kind,
            value: value.to_string(),
            hint: None,
            error: None,
        }
    }

    pub fn hint(mut self, hint: &str) -> Self {
        self.hint = Some(hint.to_string());
        self
    }

    pub fn error(mut self, error: Option<String>) -> Self {
        self.error = error;
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DialogSpec {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub submit_label: String,
    pub cancel_label: String,
    /// True for a dialog that must not be dismissed by a stray tap outside it or
    /// by the back gesture. Host key verification is what this is for.
    pub grave: bool,
    pub fields: Vec<DialogField>,
    /// Wipe the clipboard once the dialog is submitted.
    ///
    /// Set for the key import, which pulls a private key through the clipboard.
    pub clear_clipboard_on_submit: bool,
}

impl DialogSpec {
    pub fn new(title: &str) -> Self {
        Self {
            title: title.to_string(),
            message: None,
            submit_label: "OK".to_string(),
            cancel_label: "Cancel".to_string(),
            grave: false,
            fields: vec![],
            clear_clipboard_on_submit: false,
        }
    }

    pub fn message(mut self, message: &str) -> Self {
        self.message = Some(message.to_string());
        self
    }

    pub fn submit_label(mut self, label: &str) -> Self {
        self.submit_label = label.to_string();
        self
    }

    pub fn cancel_label(mut self, label: &str) -> Self {
        self.cancel_label = label.to_string();
        self
    }

    pub fn grave(mut self, grave: bool) -> Self {
        self.grave = grave;
        self
    }

    pub fn field(mut self, field: DialogField) -> Self {
        self.fields.push(field);
        self
    }

    pub fn clear_clipboard_on_submit(mut self, clear: bool) -> Self {
        self.clear_clipboard_on_submit = clear;
        self
    }

    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}

/// What the user typed.
#[derive(Debug, Default, Deserialize)]
pub struct DialogValues {
    values: HashMap<String, String>,
}

impl DialogValues {
    pub fn get(&self, key: &str) -> &str {
        self.values.get(key).map(|s| s.as_str()).unwrap_or("")
    }

    /// Take a value out, leaving nothing behind.
    ///
    /// Used for a password or a key: the answer should not stay in this map any
    /// longer than the one place that consumes it needs it.
    pub fn take(&mut self, key: &str) -> String {
        self.values.remove(key).unwrap_or_default()
    }

    pub fn parse_u16(&self, key: &str) -> Option<u16> {
        self.get(key).trim().parse().ok()
    }

    #[cfg(test)]
    pub(crate) fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        Self {
            values: pairs
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }
}

fn parse_values(payload: &str) -> anyhow::Result<DialogValues> {
    // Deliberately not logged on failure: a payload can hold a password.
    serde_json::from_str(payload)
        .map_err(|err| anyhow::anyhow!("could not read the dialog's answer: {err}"))
}

/// Present a dialog. `Ok(None)` when the user dismissed it.
pub async fn present(spec: &DialogSpec) -> anyhow::Result<Option<DialogValues>> {
    match request_dialog(spec.to_json()?).await? {
        DialogOutcome::Submitted(payload) => Ok(Some(parse_values(&payload)?)),
        DialogOutcome::Cancelled => Ok(None),
    }
}

/// Present a dialog with no fields, and report whether it was confirmed.
pub async fn confirm(spec: &DialogSpec) -> anyhow::Result<bool> {
    Ok(present(spec).await?.is_some())
}

/// True when a native dialog can be shown at all.
pub fn available() -> bool {
    window::dialog::dialogs_available()
}

/// The question `wezterm-ssh` blocks a first connection on.
///
/// Every first connection to a new host reaches this, so with a sidebar full of
/// hosts it is the common path rather than an edge case; the wording says what
/// accepting means rather than assuming the user recognises a fingerprint.
pub fn host_verify_spec(message: &str) -> DialogSpec {
    DialogSpec::new("Unknown host key")
        .message(&format!(
            "{message}\n\nAccepting adds this key to known_hosts. Only do so if you \
             recognise the fingerprint, or are connecting to this host for the \
             first time on a network you trust."
        ))
        .submit_label("Trust and connect")
        .cancel_label("Cancel")
}

/// A host key that does not match the one on record.
///
/// Grave: there is no "yes" to offer, and this is the one notice that must not be
/// dismissed by a stray tap outside it or by the back gesture.
pub fn host_verification_failed_spec(message: &str) -> DialogSpec {
    DialogSpec::new("HOST KEY CHANGED")
        .message(message)
        .submit_label("Dismiss")
        .cancel_label("Dismiss")
        .grave(true)
}

/// A password or a key passphrase.
pub fn credential_spec(prompt: &str, echo: bool) -> DialogSpec {
    // ssh prompts arrive as several lines with the question last. The leading
    // lines are instructions and belong above the field, not in its label.
    let mut lines: Vec<&str> = prompt.split('\n').collect();
    let label = lines.pop().unwrap_or("Password").trim();
    let message = lines.join("\n");

    let mut spec = DialogSpec::new("Authentication")
        .field(DialogField::new(
            CREDENTIAL_KEY,
            if label.is_empty() { "Password" } else { label },
            if echo {
                FieldKind::Text
            } else {
                FieldKind::Password
            },
            "",
        ))
        .submit_label("Continue");
    if !message.trim().is_empty() {
        spec = spec.message(message.trim());
    }
    spec
}

/// The key the credential prompt's answer arrives under.
pub const CREDENTIAL_KEY: &str = "credential";

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn a_spec_serializes_to_what_kotlin_reads() {
        let spec = DialogSpec::new("Edit host")
            .submit_label("Save")
            .field(DialogField::new("port", "Port", FieldKind::Number, "22"))
            .field(
                DialogField::new("host", "Host", FieldKind::Text, "")
                    .error(Some("cannot be empty".to_string())),
            );

        let json: serde_json::Value = serde_json::from_str(&spec.to_json().unwrap()).unwrap();
        assert_eq!(json["title"], "Edit host");
        assert_eq!(json["submit_label"], "Save");
        assert_eq!(json["grave"], false);
        assert_eq!(json["fields"][0]["kind"], "number");
        assert_eq!(json["fields"][0]["value"], "22");
        assert_eq!(json["fields"][1]["error"], "cannot be empty");
        // Absent rather than null, so Kotlin's optString gives "" and the
        // difference between "no hint" and "an empty hint" never arises.
        assert!(json["fields"][0].get("error").is_none());
        assert!(json["fields"][0].get("hint").is_none());
        assert!(json.get("message").is_none());
    }

    #[test]
    fn field_kinds_use_the_names_kotlin_matches_on() {
        // These strings are the contract with WezTermDialogs.kt; renaming a
        // variant without updating both makes the field silently fall back to
        // plain text, which for a password means it is no longer masked.
        let named = |kind: FieldKind| {
            serde_json::to_value(kind)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(named(FieldKind::Text), "text");
        assert_eq!(named(FieldKind::Number), "number");
        assert_eq!(named(FieldKind::Password), "password");
        assert_eq!(named(FieldKind::SecretMultiline), "secret_multiline");
    }

    #[test]
    fn values_are_read_back_from_the_reply() {
        let values = parse_values(r#"{"values":{"host":"a.example.com","port":"2222"}}"#).unwrap();
        assert_eq!(values.get("host"), "a.example.com");
        assert_eq!(values.parse_u16("port"), Some(2222));
        // A missing key reads as empty rather than panicking: the reply comes
        // from another process and cannot be trusted to be complete.
        assert_eq!(values.get("nope"), "");
        assert_eq!(values.parse_u16("host"), None);
    }

    #[test]
    fn taking_a_value_removes_it() {
        let mut values = parse_values(r#"{"values":{"credential":"hunter2"}}"#).unwrap();
        assert_eq!(values.take("credential"), "hunter2");
        assert_eq!(values.get("credential"), "");
        assert_eq!(values.take("credential"), "");
    }

    #[test]
    fn an_unreadable_reply_is_an_error_not_a_panic() {
        assert!(parse_values("not json").is_err());
        assert!(parse_values(r#"{"values":42}"#).is_err());
    }

    #[test]
    fn a_credential_prompt_puts_the_instructions_above_the_field() {
        // ssh sends several lines with the question last.
        let spec = credential_spec("Two-factor required\nEnter code: ", false);
        assert_eq!(spec.message.as_deref(), Some("Two-factor required"));
        assert_eq!(spec.fields[0].label, "Enter code:");
        assert_eq!(spec.fields[0].kind, FieldKind::Password);

        // And a one-line prompt has no message at all.
        let spec = credential_spec("Password: ", false);
        assert!(spec.message.is_none());
        assert_eq!(spec.fields[0].label, "Password:");

        // echo means the answer is not a secret, so it is not masked.
        assert_eq!(
            credential_spec("Name: ", true).fields[0].kind,
            FieldKind::Text
        );
    }

    #[test]
    fn a_host_key_mismatch_cannot_be_dismissed_by_accident() {
        assert!(host_verification_failed_spec("changed").grave);
        assert!(host_verification_failed_spec("changed").fields.is_empty());
        // The first-connection question is an ordinary one and is cancellable.
        assert!(!host_verify_spec("fingerprint").grave);
    }
}
