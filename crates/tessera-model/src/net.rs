use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NetId(pub u32);

impl NetId {
    /// KiCad's "no net" (net code 0) — copper that belongs to no net, e.g.
    /// an unconnected pin's pad. Verified against real `kicad-cli`: such
    /// copper gets the Default net class's clearance against every other
    /// item, *including another no-net item* — "no net" is never the same
    /// net as anything, itself included (see [`NetId::same_net`]).
    pub const NONE: Self = Self(0);

    #[must_use]
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }

    /// True iff two items are electrically the same net, so clearance
    /// between them doesn't apply. Two no-net items are *not*.
    #[must_use]
    pub const fn same_net(self, other: Self) -> bool {
        self.0 == other.0 && !self.is_none()
    }
}

/// A KiCad net: a named electrical connection. `net_class` names the
/// [`crate::NetClass`] this net is assigned to — always present, since every
/// net belongs to at least the implicit `"Default"` class in KiCad.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Net {
    pub id: NetId,
    pub name: String,
    pub net_class: String,
}
