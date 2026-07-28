use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NetId(pub u32);

/// A KiCad net: a named electrical connection. `net_class` names the
/// [`crate::NetClass`] this net is assigned to — always present, since every
/// net belongs to at least the implicit `"Default"` class in KiCad.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Net {
    pub id: NetId,
    pub name: String,
    pub net_class: String,
}
