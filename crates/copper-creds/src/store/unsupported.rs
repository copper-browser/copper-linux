//! Platforms with no backend yet (macOS has its own app and keychain code):
//! every call answers `Unavailable`, so passwords are simply off.

use super::StoreError;
use crate::credential::Login;

fn unavailable() -> StoreError {
    StoreError::Unavailable("no password store on this platform".into())
}

pub(super) struct Store {
    app: String,
}

impl Store {
    pub(super) async fn open(_tag: &str) -> Result<Self, StoreError> {
        Err(unavailable())
    }

    pub(super) fn tag(&self) -> &str {
        &self.app
    }

    pub(super) async fn logins(&self, _origin: Option<&str>) -> Result<Vec<Login>, StoreError> {
        Err(unavailable())
    }

    pub(super) async fn secret(
        &self,
        _host: &str,
        _user: &str,
    ) -> Result<Option<String>, StoreError> {
        Err(unavailable())
    }

    pub(super) async fn save(
        &self,
        _host: &str,
        _user: &str,
        _password: &str,
        _used: Option<f64>,
    ) -> Result<(), StoreError> {
        Err(unavailable())
    }

    pub(super) async fn touch(
        &self,
        _host: &str,
        _user: &str,
        _now: f64,
    ) -> Result<(), StoreError> {
        Err(unavailable())
    }

    pub(super) async fn forget(&self, _host: &str, _user: &str) -> Result<(), StoreError> {
        Err(unavailable())
    }

    pub(super) async fn forget_origin(&self, _host: &str) -> Result<usize, StoreError> {
        Err(unavailable())
    }
}

pub(super) async fn lookup(_attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>, StoreError> {
    Err(unavailable())
}

pub(super) async fn store_raw(
    _label: &str,
    _attributes: &[(&str, &str)],
    _secret: &[u8],
) -> Result<(), StoreError> {
    Err(unavailable())
}
