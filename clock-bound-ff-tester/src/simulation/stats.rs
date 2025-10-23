//! Code specific to statistical tools used to model oscillators, NTP events, and other time
//! related phenomena.

use rand::distributions::Distribution as RandDistribution;

/// Our internal Distribution trait wrapping the `rand::distributions::Distribution` trait which
/// implements sampling as well as several other Traits required by internal modules.
///
/// At it's core this trait is a convince trait intended to ease the burden of adding statistical distributions to objects
/// which must include `Copy`, `PartialEq` traits. Additionally, we currently sample only f64 values
/// which this trait makes explicit.
pub trait Distribution: RandDistribution<f64> + Copy + PartialEq + std::fmt::Debug {}
impl<T> Distribution for T where T: RandDistribution<f64> + Copy + PartialEq + std::fmt::Debug {}

mod gamma;
pub use gamma::GammaDistribution;

mod normal;
pub use normal::NormalDistribution;

mod truncated;
pub use truncated::Truncated;

pub use statrs::distribution::Dirac as DiracDistribution;

pub mod string_parse;
