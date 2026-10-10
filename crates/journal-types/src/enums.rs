//! Numeric enums of the wire format. Every one rejects unknown bytes in `TryFrom<u8>`.

/// A byte that is not a defined value of the enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownValue;

macro_rules! u8_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $var:ident = $val:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        pub enum $name { $($(#[$vmeta])* $var = $val),+ }

        impl $name {
            /// The wire byte.
            pub const fn as_u8(self) -> u8 {
                self as u8
            }
        }

        impl TryFrom<u8> for $name {
            type Error = UnknownValue;
            fn try_from(v: u8) -> Result<Self, UnknownValue> {
                match v {
                    $($val => Ok($name::$var),)+
                    _ => Err(UnknownValue),
                }
            }
        }
    };
}

u8_enum! {
    /// Position side. Wire: Long 0, Short 1.
    Side { Long = 0, Short = 1 }
}
u8_enum! {
    /// Order kind of a `Fill`. Wire: Market 0, Limit 1, Stop 2.
    OrderKind { Market = 0, Limit = 1, Stop = 2 }
}
u8_enum! {
    /// Why a position slice closed. Wire: Manual 0, StopLoss 1, TakeProfit 2, Flip 3, Breach 4, Fail 5.
    CloseReason { Manual = 0, StopLoss = 1, TakeProfit = 2, Flip = 3, Breach = 4, Fail = 5 }
}
u8_enum! {
    /// Fee kind. Wire: Holding 0, Commission 1.
    FeeKind { Holding = 0, Commission = 1 }
}
u8_enum! {
    /// Reason of a `Fail` event. Wire: StaticFloor 0, PhaseQuick 1.
    FailReason { StaticFloor = 0, PhaseQuick = 1 }
}
