//! One saved sign-in on its way into Copper's password store, whichever
//! importer found it ([`crate::passwords`] on Linux, [`crate::passwords_csv`]
//! everywhere).

/// One saved sign-in, in the clear.
#[derive(Clone, PartialEq)]
pub struct ImportedLogin {
    /// The site (lowercased, no `www.`), as Copper keys passwords.
    pub host: String,
    pub user: String,
    pub password: String,
    /// Last used, Unix seconds.
    pub used: Option<f64>,
}

impl std::fmt::Debug for ImportedLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportedLogin")
            .field("host", &self.host)
            .field("user", &self.user)
            .field("used", &self.used)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_the_password() {
        let login = ImportedLogin {
            host: "h".into(),
            user: "u".into(),
            password: "SECRET".into(),
            used: None,
        };
        assert!(!format!("{login:?}").contains("SECRET"));
    }
}
