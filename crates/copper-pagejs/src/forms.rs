//! Forms and passwords (Forms.swift `FormRelay`, Tab.swift): the relay user
//! script that installs `window.__officeForms` and posts to `officeForms`,
//! and the calls into it. Filling goes through the native value setter and
//! fires input/change/keyup/blur. Copper never fills on its own: these run
//! only when the user (or an agent with access) asks.

use std::collections::BTreeMap;

use serde_json::json;

use crate::call::{Call, UserScript};
use crate::script::Script;

/// The relay: document end, main frame.
pub fn relay() -> UserScript {
    UserScript::new(Script::FormsRelay, json!({}))
}

/// Takes `PublicKeyCredential` away so sites go straight to the password:
/// document start, all frames. Install only while passkeys are not offered.
pub fn without_passkeys() -> UserScript {
    UserScript::new(Script::WithoutPasskeys, json!({}))
}

/// Field groups `hasFields` understands.
pub const GROUPS: [&str; 4] = ["login", "card", "identity", "other"];

/// Fill the sign-in pair (the name only when its box is empty). `None` when
/// the relay is not on the page.
pub fn fill_login(user: &str, password: &str) -> Call<Option<bool>> {
    Call::new(
        Script::FormsFillLogin,
        json!({ "user": user, "password": password }),
    )
}

/// Press Enter in the password box and submit its form (or click the nearest button).
pub fn submit() -> Call<Option<bool>> {
    Call::new(Script::FormsSubmit, json!({}))
}

pub fn fill_otp(code: &str) -> Call<Option<bool>> {
    Call::new(Script::FormsFillOtp, json!({ "code": code }))
}

/// Fill by field kind (`firstName`, `email`, `cardNumber`, `cardExp`, …);
/// answers how many boxes were filled.
pub fn fill_values(values: &BTreeMap<String, String>) -> Call<Option<u32>> {
    Call::new(Script::FormsFillValues, json!({ "values": values }))
}

pub fn fill_focused(value: &str) -> Call<Option<bool>> {
    Call::new(Script::FormsFillFocused, json!({ "value": value }))
}

/// Fill the box whose label matches `label` (custom vault fields).
pub fn fill_field(label: &str, value: &str) -> Call<Option<bool>> {
    Call::new(
        Script::FormsFillField,
        json!({ "label": label, "value": value }),
    )
}

pub fn has_fields(group: &str) -> Call<bool> {
    Call::new(Script::FormsHasFields, json!({ "group": group }))
}

/// Field kinds present, first occurrence order.
pub fn fields_present() -> Call<Vec<String>> {
    Call::new(Script::FormsFieldsPresent, json!({}))
}

pub fn has_otp() -> Call<bool> {
    Call::new(Script::FormsHasOtp, json!({}))
}

pub fn has_password() -> Call<bool> {
    Call::new(Script::FormsHasPassword, json!({}))
}

/// Whether typed, unsent text would be lost by putting the tab to sleep.
pub fn unsaved() -> Call<bool> {
    Call::new(Script::FormsUnsaved, json!({}))
}
