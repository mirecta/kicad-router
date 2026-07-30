#![forbid(unsafe_code)]

mod board;
mod connection;
mod layer;
mod net;
mod net_class;
mod pad;
mod rule_area;
mod track;
mod via;

pub use board::Board;
pub use connection::{Connection, ConnectionReport, Endpoint};
pub use layer::{Layer, LayerId, LayerKind};
pub use net::{Net, NetId};
pub use net_class::NetClass;
pub use pad::{Pad, PadId, PadShape};
pub use rule_area::{KeepoutFlags, RuleArea, RuleAreaId};
pub use track::{Track, TrackId};
pub use via::{Via, ViaId};
