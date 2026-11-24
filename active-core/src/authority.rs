//! L-P35: one authority vocabulary with non-widening attenuation.
//!
//! Five authorities cover the real hand-off surfaces: evaluate,
//! read-secrets, inspect, apply, sign. The set is a bitset; wire names are
//! the lowercase kebab spellings. A profile name is never authority.

use std::fmt;

/// A content authority. Closed set; adding one is a spec change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    Evaluate,
    ReadSecrets,
    Inspect,
    Apply,
    Sign,
}

impl Capability {
    pub const ALL: [Capability; 5] = [
        Capability::Evaluate,
        Capability::ReadSecrets,
        Capability::Inspect,
        Capability::Apply,
        Capability::Sign,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Evaluate => "evaluate",
            Capability::ReadSecrets => "read-secrets",
            Capability::Inspect => "inspect",
            Capability::Apply => "apply",
            Capability::Sign => "sign",
        }
    }
}

impl std::str::FromStr for Capability {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "evaluate" => Ok(Capability::Evaluate),
            "read-secrets" => Ok(Capability::ReadSecrets),
            "inspect" => Ok(Capability::Inspect),
            "apply" => Ok(Capability::Apply),
            "sign" => Ok(Capability::Sign),
            _ => Err(format!("unknown capability `{text}`")),
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A set of authorities. Ordered iteration is by `Capability::ALL`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CapabilitySet(u8);

impl CapabilitySet {
    pub const fn none() -> Self {
        Self(0)
    }

    pub const fn all() -> Self {
        Self(0b1_1111)
    }

    fn bit(capability: Capability) -> u8 {
        1 << capability as u8
    }

    pub fn contains(self, capability: Capability) -> bool {
        self.0 & Self::bit(capability) != 0
    }

    pub fn insert(&mut self, capability: Capability) {
        self.0 |= Self::bit(capability);
    }

    pub fn remove(&mut self, capability: Capability) {
        self.0 &= !Self::bit(capability);
    }

    pub fn is_subset(self, other: Self) -> bool {
        self.0 & !other.0 == 0
    }

    pub fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Capability> {
        Capability::ALL
            .into_iter()
            .filter(move |capability| self.contains(*capability))
    }

    /// First authority in `self` that `other` does not have.
    pub fn first_missing(self, other: Self) -> Option<Capability> {
        self.iter().find(|capability| !other.contains(*capability))
    }

    /// Sorted wire names (lexicographic), stable output.
    pub fn wire_names(self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.iter().map(Capability::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Parse a list of wire names; duplicates and unknown names are errors
    /// (a set is not ambiguous).
    pub fn parse_list<'a, I>(names: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut set = Self::none();
        for name in names {
            let capability: Capability = name.parse()?;
            if set.contains(capability) {
                return Err(format!("duplicate capability `{name}`"));
            }
            set.insert(capability);
        }
        Ok(set)
    }
}

impl fmt::Display for CapabilitySet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.wire_names().join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subset_and_missing_name() {
        let held = CapabilitySet::all();
        let granted = CapabilitySet::parse_list(["evaluate"]).unwrap();
        assert!(granted.is_subset(held));
        let missing = CapabilitySet::parse_list(["apply"]).unwrap();
        assert_eq!(missing.first_missing(granted), Some(Capability::Apply));
    }

    #[test]
    fn wire_order_is_sorted_and_duplicates_refused() {
        let set = CapabilitySet::parse_list(["sign", "evaluate"]).unwrap();
        assert_eq!(set.wire_names(), vec!["evaluate", "sign"]);
        assert!(CapabilitySet::parse_list(["evaluate", "evaluate"]).is_err());
        assert!(CapabilitySet::parse_list(["admin"]).is_err());
    }
}
