//! NTP related stuff

mod dispersion_increase_linear;
mod multi_source;
mod perfect;
mod series;
mod variable_network_delay_source;

use crate::simulation::generator::Generator;

pub use dispersion_increase_linear::{
    Generator as DispersionIncreaseLinearGenerator, Props as DispersionIncreaseLinearGeneratorProps,
};
pub use multi_source::{
    Generator as MultiSourceGenerator, GeneratorBuilder as MultiSourceGeneratorBuilder,
};
pub use perfect::{Generator as PerfectGenerator, Props as PerfectGeneratorProps};
pub use series::{Generator as SeriesGenerator, Props as SeriesGeneratorProps};
pub use variable_network_delay_source::{
    Generator as VariableNetworkDelayGenerator, Props as VariableNetworkDelayGeneratorProps,
};

mod round_trip_delays;
pub use round_trip_delays::{RoundTripDelays, VariableRoundTripDelays};
