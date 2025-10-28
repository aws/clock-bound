//! The NTP Feed-forward time synchronization algorithm

use std::num::NonZeroUsize;

use super::event_buffer;
use crate::daemon::{
    clock_parameters::ClockParameters,
    clock_sync_algorithm::ring_buffer::Quarter,
    event::{self, TscRtt},
    time::tsc::Period,
};

/// Feed forward time synchronization algorithm for a single NTP source
#[derive(Debug, Clone)]
pub struct Ntp {
    /// Events within the current SKM (within 1024 seconds)
    local: event_buffer::Local<event::Ntp>,
    /// Best RTT values of each SKM over the last week
    estimate: event_buffer::Estimate<event::Ntp>,
    /// Current calculation of [`ClockParameters`]
    clock_parameters: Option<ClockParameters>,
    /// Current TSC period estimate
    period_estimate: Option<Period>,
}

impl Ntp {
    /// Create a new feed forward time synchronization algorithm
    ///
    /// `local_capacity` should be the number of data-points to span an SKM window.
    /// For example, if the source is expected to sample once every second, the `local_capacity`
    /// should have a max value of 1024.
    pub fn new(_local_capacity: NonZeroUsize) -> Self {
        todo!()
        // Self {
        //     local: event_buffer::Local::new(local_capacity),
        //     estimate: event_buffer::Estimate::new(),
        //     clock_parameters: None,
        //     period_estimate: None,
        // }
    }

    /// Feed an event into this algorithm
    ///
    /// Returns [`Some`] if the event has improved this source's [`ClockParameters`].
    #[expect(clippy::needless_pass_by_value, reason = "impl will remove this lint")]
    pub fn feed(&mut self, event: event::Ntp) -> Option<ClockParameters> {
        tracing::debug!(?event, "feed");
        None
    }

    /// Get the current [`ClockParameters`]
    pub fn clock_parameters(&self) -> Option<&ClockParameters> {
        self.clock_parameters.as_ref()
    }

    /// Calculate the local period and associated error
    ///
    /// Returns `None` if `local` has less than 2 data points
    #[expect(
        clippy::cast_precision_loss,
        reason = "Needed an escape hatch. Error units weren't lining up"
    )]
    fn calculate_local_period_and_error(
        local: &event_buffer::Local<event::Ntp>,
    ) -> Option<LocalPeriodAndError> {
        if local.as_ref().len() < 2 {
            return None;
        }
        // unwrap okay, length of 2 means both quarters and min check will succeed
        let old = local.as_ref().min_rtt_in_quarter(Quarter::Oldest).unwrap();
        let new = local.as_ref().min_rtt_in_quarter(Quarter::Newest).unwrap();

        let period_local = new.calculate_period_backward(old);

        // unwrap okay, local is not empty
        let min = local.as_ref().min_rtt().unwrap();
        let min_rtt = min.rtt();
        let new_sample_error = new.rtt() - min_rtt;
        let old_sample_error = old.rtt() - min_rtt;

        assert!(new_sample_error.get() >= 0);
        assert!(old_sample_error.get() >= 0);

        // We have used this somewhere else, but I don't see the math for this.
        // units also come out as GHz... (ticks / nanoseconds)
        // seems sus...
        //
        // Currently this value is not used within the FF algorithm nor client, but we need to address this calculation
        // in the future
        let error = ((new_sample_error + old_sample_error).get() as f64)
            / (new.data().server_recv_time - old.data().server_recv_time).as_nanos() as f64;
        let error = Period::from_seconds(error);
        tracing::debug!(
            ?old,
            ?new,
            min_rtt = ?min,
            %period_local,
            %error,
            "Calculated local period and error"
        );
        Some(LocalPeriodAndError {
            period_local,
            error,
        })
    }
}

/// Used as the output for [`Ntp::calculate_local_period_and_error`]
struct LocalPeriodAndError {
    /// period calculation
    period_local: Period,
    /// period error
    error: Period,
}

#[cfg(test)]
mod test {
    use crate::daemon::{
        event::{NtpData, Stratum},
        time::{Duration, Instant, TscCount},
    };

    use super::*;

    #[test]
    fn calculate_local_period_returns_none() {
        // return none if local has < 2 events

        let mut local = event_buffer::Local::new(NonZeroUsize::new(2).unwrap());
        let result = Ntp::calculate_local_period_and_error(&local);
        assert!(result.is_none());

        local
            .feed(
                event::Ntp::builder()
                    .tsc_pre(TscCount::new(0))
                    .tsc_post(TscCount::new(1_000))
                    .ntp_data(NtpData {
                        server_recv_time: Instant::from_days(1),
                        server_send_time: Instant::from_days(1) + Duration::from_micros(5),
                        root_delay: Duration::from_micros(6),
                        root_dispersion: Duration::from_nanos(350),
                        stratum: Stratum::TWO,
                    })
                    .build()
                    .unwrap(),
            )
            .unwrap();

        let result = Ntp::calculate_local_period_and_error(&local);
        assert!(result.is_none());
    }

    #[test]
    fn calculate_local_period() {
        let mut local = event_buffer::Local::new(NonZeroUsize::new(2).unwrap());

        let old_event = event::Ntp::builder()
            .tsc_pre(TscCount::new(0))
            .tsc_post(TscCount::new(1_000_000))
            .ntp_data(NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(1) + Duration::from_micros(5),
                root_delay: Duration::from_micros(6),
                root_dispersion: Duration::from_nanos(350),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        // new event is 100 seconds in the future with tsc at 1GHz
        let new_event = event::Ntp::builder()
            .tsc_pre(TscCount::new(100_000_000_000))
            .tsc_post(TscCount::new(100_001_000_000))
            .ntp_data(NtpData {
                server_recv_time: Instant::from_days(1) + Duration::from_secs(100),
                server_send_time: Instant::from_days(1)
                    + Duration::from_secs(100)
                    + Duration::from_micros(5),
                root_delay: Duration::from_micros(6),
                root_dispersion: Duration::from_nanos(350),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        local.feed(old_event).unwrap();
        local.feed(new_event).unwrap();

        let result = Ntp::calculate_local_period_and_error(&local).unwrap();
        assert_eq!(result.period_local, Period::from_seconds(1.0e-9));
    }
}
