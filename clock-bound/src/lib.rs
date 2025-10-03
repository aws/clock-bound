//! ClockBound

#[cfg(feature = "client")]
pub mod client;

pub mod shm;

pub mod vmclock;

#[cfg(feature = "daemon")]
pub mod daemon;

#[cfg(feature = "daemon")] // can open this up if we ever use this in the client
mod private {
    // define a crate sealed trait
    // https://rust-lang.github.io/api-guidelines/future-proofing.html#sealed-traits-protect-against-downstream-implementations-c-sealed
    pub trait Sealed {}
}
