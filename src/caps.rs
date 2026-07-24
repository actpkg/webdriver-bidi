//! Semantic `browser:*` capability classes.
//!
//! TODO(act-consent): these are self-enforced in-component today. When the
//! `act:consent` WIT lands, the host becomes the enforcement point and this
//! module reduces to a declaration. Self-reported enforcement belongs in the
//! same trust category as `[std.hints]` — it is attested, not observed.

use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum BrowserCap {
    Navigate,
    Script,
    Input,
    Read,
}

impl BrowserCap {
    pub fn id(&self) -> &'static str {
        match self {
            BrowserCap::Navigate => "browser:navigate",
            BrowserCap::Script => "browser:script",
            BrowserCap::Input => "browser:input",
            BrowserCap::Read => "browser:read",
        }
    }
}

pub struct CapSet(Vec<BrowserCap>);

impl CapSet {
    pub fn all() -> Self {
        CapSet(vec![
            BrowserCap::Navigate,
            BrowserCap::Script,
            BrowserCap::Input,
            BrowserCap::Read,
        ])
    }

    pub fn from_list(list: Vec<BrowserCap>) -> Self {
        CapSet(list)
    }

    pub fn require(&self, cap: BrowserCap) -> Result<(), String> {
        if self.0.contains(&cap) {
            Ok(())
        } else {
            Err(format!(
                "capability {} is not granted to this session",
                cap.id()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_permits_everything() {
        let c = CapSet::all();
        assert!(c.require(BrowserCap::Navigate).is_ok());
        assert!(c.require(BrowserCap::Script).is_ok());
        assert!(c.require(BrowserCap::Input).is_ok());
        assert!(c.require(BrowserCap::Read).is_ok());
    }

    #[test]
    fn restricted_set_denies_absent_cap() {
        let c = CapSet::from_list(vec![BrowserCap::Navigate, BrowserCap::Read]);
        assert!(c.require(BrowserCap::Navigate).is_ok());
        let e = c.require(BrowserCap::Script).unwrap_err();
        assert!(e.contains("browser:script"));
    }

    #[test]
    fn empty_set_denies_everything() {
        let c = CapSet::from_list(vec![]);
        assert!(c.require(BrowserCap::Read).is_err());
    }

    #[test]
    fn ids_match_act_toml_declarations() {
        assert_eq!(BrowserCap::Navigate.id(), "browser:navigate");
        assert_eq!(BrowserCap::Script.id(), "browser:script");
        assert_eq!(BrowserCap::Input.id(), "browser:input");
        assert_eq!(BrowserCap::Read.id(), "browser:read");
    }

    #[test]
    fn deserializes_from_kebab_case_names() {
        let list: Vec<BrowserCap> =
            serde_json::from_str(r#"["navigate","read"]"#).expect("kebab-case names parse");
        assert_eq!(list, vec![BrowserCap::Navigate, BrowserCap::Read]);
    }
}
