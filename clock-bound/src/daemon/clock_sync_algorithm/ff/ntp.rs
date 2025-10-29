//! The NTP Feed-forward time synchronization algorithm

use std::num::NonZeroUsize;

use super::event_buffer;
use crate::daemon::{
    clock_parameters::ClockParameters,
    clock_sync_algorithm::{ff::UncorrectedClock, ring_buffer::Quarter},
    event::{self, TscRtt},
    time::{
        Duration, TscDiff,
        tsc::{Period, Skew},
    },
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

    /// Calculate the estimate period and k value based off of the ring buffers
    ///
    /// Returns `None` if we do not have enough data points to calculate a period.
    ///
    /// We calculate a period by finding a pair of RTT values that are recent and as far in the past as possible,
    /// and using the pair of TSC/reference clock values,
    ///
    /// ## Steady State
    /// While the program is running, it is expected that the `Local` event buffer is completely filled, and the
    /// `Estimate` event buffer is partially/completely filled. In this state, the "new" value comes from the min value
    /// in the current SKM window (local buffer), and "old" is the oldest datapoint in the estimate buffer.
    ///
    /// ## Initializing
    /// If there are less than 2 values in the estimate buffer, then use the min values in the local buffer
    /// for "new" and "old" values.
    ///
    /// ## Starvation
    /// If local buffer is empty but we have multiple values in estimate, we can just use those.
    fn calculate_uncorrected_clock(
        local: &event_buffer::Local<event::Ntp>,
        estimate: &event_buffer::Estimate<event::Ntp>,
    ) -> Option<UncorrectedClock> {
        // get the min RTT values in the "old" and "new" time ranges
        let (oldest, newest) = if estimate.as_ref().len() < 2 {
            // fallback to local only calculation if estimate is small.
            //
            // rationale for not using "is empty" logic: When there is a single
            // value in the estimate buffer, it will end up being the min value
            // in the local buff for some time. Best to avoid that.

            // old means it's in the oldest quarter of the local buffer
            // new means it's in the newest quarter of the local buffer
            if local.as_ref().len() < 2 {
                // We need at least 2 data points to start estimating the period
                return None;
            }
            let oldest = local.as_ref().min_rtt_in_quarter(Quarter::Oldest).unwrap();
            let newest = local.as_ref().min_rtt_in_quarter(Quarter::Newest).unwrap();
            (oldest, newest)
        } else if local.is_empty() {
            // If local is empty but we have values in estimate (can happen if we are starving),
            // we can fallback to using the estimate buffer
            // unwrap okay. Estimate buffer has at least 2 data points from above if statement
            let oldest = estimate.as_ref().tail().unwrap();
            let newest = estimate.as_ref().head().unwrap();
            (oldest, newest)
        } else {
            // happy case: We have values in the estimate buffer, and values in local
            // old is the oldest value in the estimate buffer
            // new is the min rtt value of local buffer.
            // unwraps okay, if conditions above check that this will never happen
            let oldest = estimate.as_ref().tail().unwrap();
            let newest = local.as_ref().min_rtt().unwrap();
            (oldest, newest)
        };

        // calculate using the backward offset (tsc_post and server_send)
        //
        // When it comes to the time period calculation, the most important factor is network path consistency.
        // We have 4 options for doing this right now.
        // - Using the return path only (what this code does)
        // - Using the forward path only
        // - using the midpoint
        // - something more complex using period_max_error calculations
        //
        // The first 2 options (forward or return path) are stable and simple approaches.
        //
        // The midpoint creates an "averaging" effect. Since the goal is to minimize round trip, it ends
        // up creating a "worst of both worlds" scenario.
        //
        // Anecdotally, we have seen some EC2 instances have more consistent link local RTT on the return
        // path as opposed to the forward path.
        //
        // A complex strategy we could take on would be to calculate error values on slopes and directly compare
        // these values in search. That kind of approach is TBD.
        let p_estimate = (newest.data().server_send_time - oldest.data().server_send_time)
            / (newest.tsc_post() - oldest.tsc_post());

        let k = p_estimate * TscDiff::new(newest.tsc_post().get());
        let k = newest.data().server_send_time - k;

        tracing::debug!(
            ?oldest,
            ?newest,
            ?p_estimate,
            ?k,
            "Calculated period and k values"
        );

        Some(UncorrectedClock { p_estimate, k })
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

    /// Calculate the theta value, which is the time correction to be applied
    ///
    /// The theta corresponds to the equation below
    ///
    /// `C(t) = TSC(t) × p^ + K − θ^(t)` where:
    /// - `C(t)` is the absolute time. Corrected.
    /// - `TSC(t)` is the tsc reading at a time
    /// - `p^` is the estimation of the clock period
    /// - `K` is the "epoch" (the uncorrected time at `TSC(0)`)
    /// - `θ^(t)` is the time correction
    ///
    /// # Panics
    /// Panics if `Local` is empty
    #[expect(
        clippy::cast_precision_loss,
        reason = "exp and weight require floats. Values are small enough to not lose precision"
    )]
    fn calculate_theta(
        local: &event_buffer::Local<event::Ntp>,
        period_local: Period,
        uncorrected_clock: UncorrectedClock,
    ) -> CalculateThetaOutput {
        // Feed-forward time synchronization algorithm's error normalization factor.
        // This constant is used to penalize the feed_forward_samples that have a
        // slower reference clock read duration.
        const ERROR_NORMALIZATION_FACTOR: f64 = 1e5;

        assert!(!local.is_empty());

        let now_post = local.as_ref().head().unwrap().tsc_post();
        let skew = Skew::from_ratio(period_local, uncorrected_clock.p_estimate);
        let mut numerator = 0.0;
        let mut denominator = 0.0;

        let min_event = local.as_ref().min_rtt().unwrap();

        let mut max_ceb = Duration::from_secs(0);

        for event in local.iter() {
            if event.rtt() > (min_event.rtt() * local.rtt_threshold_multiplier()) {
                tracing::trace!(?event, ?min_event, "skipping event due to rtt threshold");
                continue;
            }
            // Use the worst CEB in calculation as the algorithm's CEB
            max_ceb = std::cmp::max(max_ceb, event.calculate_clock_error_bound(period_local));

            // calculate midpoints on client and server side

            let offset = event.calculate_offset(uncorrected_clock);

            // estimate error based off of TSC rtt
            let sample_error = event.rtt() - min_event.rtt();
            assert!(sample_error.get() >= 0, "cannot have a negative error");

            // weight is e^(-sqrt(Error_{i}/E))
            // escaping into f64 to minimize rounding error. Sticking with nanoseconds as the base unit
            // NOTE/FIXME: is there a world where we take into account either age of the event or the
            // clock_error_bound?
            let weight = -((sample_error.get() as f64 / ERROR_NORMALIZATION_FACTOR).sqrt());
            let weight = weight.exp();

            let offset_nsec = offset.as_seconds_f64() * 1e9;

            let skew_correction_seconds = skew.get()
                * uncorrected_clock.p_estimate.get()
                * ((now_post - event.tsc_post()).get() as f64);
            let skew_correction_nsec = skew_correction_seconds * 1e9;
            numerator += weight * (offset_nsec + skew_correction_nsec);
            denominator += weight;
        }

        let theta_nsec = numerator / denominator;
        let theta = Duration::from_seconds_f64(theta_nsec / 1e9);
        tracing::debug!(?theta, ?uncorrected_clock, %period_local, "Calculated theta");
        CalculateThetaOutput {
            theta,
            clock_error_bound: max_ceb,
        }
    }
}

/// Used as the output for [`Ntp::calculate_local_period_and_error`]
struct LocalPeriodAndError {
    /// period calculation
    period_local: Period,
    /// period error
    error: Period,
}

/// Output from [`Ntp::calculate_theta`]
struct CalculateThetaOutput {
    /// The time correction to be applied
    theta: Duration,
    /// The worst clock error bound used in calculation
    clock_error_bound: Duration,
}

#[cfg(test)]
mod tests {
    use crate::daemon::{
        clock_sync_algorithm::{
            RingBuffer,
            ff::event_buffer::{Estimate, Local},
        },
        event::{NtpData, Stratum},
        time::{Duration, Instant, TscCount},
    };

    use super::*;

    #[test]
    fn empty_buffers() {
        let local = Local::new(NonZeroUsize::new(1).unwrap());
        let estimate = Estimate::new();
        let result = Ntp::calculate_uncorrected_clock(&local, &estimate);
        assert!(result.is_none());
    }

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

    #[test]
    fn calculate_uncorrected_local_has_single_value() {
        let mut local = Local::new(NonZeroUsize::new(1).unwrap());
        let event = event::Ntp::builder()
            .tsc_pre(TscCount::new(100))
            .tsc_post(TscCount::new(110))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(1) + Duration::from_micros(10),
                root_delay: Duration::from_micros(15),
                root_dispersion: Duration::from_micros(17),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        local.feed(event.clone()).unwrap();
        let estimate = Estimate::new();

        // estimate is empty, local has a single value. Cannot calculate a period with a single datapoint
        let result = Ntp::calculate_uncorrected_clock(&local, &estimate);
        assert!(result.is_none());
    }

    #[test]
    fn calculate_uncorrected_local_has_two_values() {
        // Create 2 points that are 1 second apart and the TSC is roughly 1 GHz
        let mut local = Local::new(NonZeroUsize::new(2).unwrap());
        let server_send_time = Instant::from_days(1) + Duration::from_nanos(900);
        let event1 = event::Ntp::builder()
            .tsc_pre(TscCount::new(1_000_000_000))
            .tsc_post(TscCount::new(1_000_001_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1) + Duration::from_nanos(100),
                server_send_time,
                root_delay: Duration::from_micros(15),
                root_dispersion: Duration::from_micros(17),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        let event2 = event::Ntp::builder()
            .tsc_pre(TscCount::new(2_000_000_000))
            .tsc_post(TscCount::new(2_000_001_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1)
                    + Duration::from_secs(1)
                    + Duration::from_nanos(100),
                server_send_time: Instant::from_days(1)
                    + Duration::from_secs(1)
                    + Duration::from_nanos(900),
                root_delay: Duration::from_micros(15),
                root_dispersion: Duration::from_micros(17),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        local.feed(event1.clone()).unwrap();
        local.feed(event2.clone()).unwrap();
        let estimate = Estimate::new();

        let result = Ntp::calculate_uncorrected_clock(&local, &estimate).unwrap();

        approx::assert_abs_diff_eq!(result.p_estimate.get(), 1e-9);
        let expected = server_send_time - Duration::from_secs(1) - Duration::from_micros(1); // account for tsc_post time of 1_000_001_000 at 1GHz period
        assert_eq!(result.k, expected,);
    }

    #[test]
    fn calculate_uncorrected_estimate_has_values() {
        // Create 2 points that are 1 second apart and the TSC is roughly 1 GHz
        let mut local = Local::new(NonZeroUsize::new(2).unwrap());
        let local_event = event::Ntp::builder()
            .tsc_pre(TscCount::new(2_000_000_000_000))
            .tsc_post(TscCount::new(2_000_000_001_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1)
                    + Duration::from_secs(1999)
                    + Duration::from_nanos(100),
                server_send_time: Instant::from_days(1)
                    + Duration::from_secs(1999)
                    + Duration::from_nanos(900),
                root_delay: Duration::from_micros(15),
                root_dispersion: Duration::from_micros(17),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        local.feed(local_event).unwrap();

        let estimate_event_1 = event::Ntp::builder()
            .tsc_pre(TscCount::new(1_000_000_000))
            .tsc_post(TscCount::new(1_000_001_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1) + Duration::from_nanos(100),
                server_send_time: Instant::from_days(1) + Duration::from_nanos(900),
                root_delay: Duration::from_micros(15),
                root_dispersion: Duration::from_micros(17),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        // larger RTT
        let estimate_event_2 = event::Ntp::builder()
            .tsc_pre(TscCount::new(1_000_000_000_000))
            .tsc_post(TscCount::new(1_000_000_002_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1)
                    + Duration::from_secs(1000)
                    + Duration::from_nanos(100),
                server_send_time: Instant::from_days(1)
                    + Duration::from_secs(1000)
                    + Duration::from_nanos(100),
                root_delay: Duration::from_micros(15),
                root_dispersion: Duration::from_micros(17),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        let mut estimate_inner = RingBuffer::new(NonZeroUsize::new(5).unwrap());
        estimate_inner.push(estimate_event_1);
        estimate_inner.push(estimate_event_2);

        let estimate = Estimate::builder().inner(estimate_inner).build();

        let result = Ntp::calculate_uncorrected_clock(&local, &estimate).unwrap();

        approx::assert_abs_diff_eq!(result.p_estimate.get(), 1e-9);
        assert_eq!(
            result.k,
            Instant::from_days(1) + Duration::from_nanos(900)
                - Duration::from_secs(1)
                - Duration::from_micros(1)
        );
    }

    #[test]
    fn theta_naive() {
        // Naive test case, estimate and local periods are the same
        // clock rates 1GHz
        // epoch is 1 day after 1970 new years

        let uncorrected_clock = UncorrectedClock {
            k: Instant::from_days(1),
            p_estimate: Period::from_seconds(1e-9),
        };
        let local_period = Period::from_seconds(1e-9);

        let mut local = Local::new(NonZeroUsize::new(2).unwrap());
        let event1 = event::Ntp::builder()
            .tsc_pre(TscCount::new(1_000_000_000))
            .tsc_post(TscCount::new(1_000_001_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1)
                    + Duration::from_secs(1)
                    + Duration::from_nanos(100),
                server_send_time: Instant::from_days(1)
                    + Duration::from_secs(1)
                    + Duration::from_nanos(900),
                root_delay: Duration::from_micros(15),
                root_dispersion: Duration::from_micros(17),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        let event2 = event::Ntp::builder()
            .tsc_pre(TscCount::new(2_000_000_000))
            .tsc_post(TscCount::new(2_000_001_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1)
                    + Duration::from_secs(2)
                    + Duration::from_nanos(100),
                server_send_time: Instant::from_days(1)
                    + Duration::from_secs(2)
                    + Duration::from_nanos(900),
                root_delay: Duration::from_micros(15),
                root_dispersion: Duration::from_micros(17),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        local.feed(event1.clone()).unwrap();
        local.feed(event2.clone()).unwrap();

        let CalculateThetaOutput {
            theta,
            clock_error_bound,
        } = Ntp::calculate_theta(&local, local_period, uncorrected_clock);

        assert_eq!(theta, Duration::from_secs(0));
        assert_eq!(
            clock_error_bound,
            event1.calculate_clock_error_bound(local_period)
        );
    }

    #[test]
    fn theta() {
        // Less naive test case, estimate and local periods are 10 PPM off
        // estimate clock rates 1GHz
        // epoch is 1 day after 1970 new years

        let uncorrected_clock = UncorrectedClock {
            k: Instant::from_days(1),
            p_estimate: Period::from_seconds(1e-9),
        };
        let local_period = Period::from_seconds(uncorrected_clock.p_estimate.get() * 1.000_010);

        let mut local = Local::new(NonZeroUsize::new(2).unwrap());
        let event1 = event::Ntp::builder()
            .tsc_pre(TscCount::new(1_000_000_000))
            .tsc_post(TscCount::new(1_000_001_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1)
                    + Duration::from_secs(1)
                    + Duration::from_nanos(100),
                server_send_time: Instant::from_days(1)
                    + Duration::from_secs(1)
                    + Duration::from_nanos(900),
                root_delay: Duration::from_micros(15),
                root_dispersion: Duration::from_micros(27),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        let event2 = event::Ntp::builder()
            .tsc_pre(TscCount::new(2_000_000_000))
            .tsc_post(TscCount::new(2_000_001_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1)
                    + Duration::from_secs(2)
                    + Duration::from_nanos(100),
                server_send_time: Instant::from_days(1)
                    + Duration::from_secs(2)
                    + Duration::from_nanos(900),
                root_delay: Duration::from_micros(15),
                root_dispersion: Duration::from_micros(17),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        local.feed(event1.clone()).unwrap();
        local.feed(event2.clone()).unwrap();

        let CalculateThetaOutput {
            theta,
            clock_error_bound,
        } = Ntp::calculate_theta(&local, local_period, uncorrected_clock);

        assert_eq!(theta, Duration::from_micros(-5));
        assert_eq!(
            clock_error_bound,
            event1.calculate_clock_error_bound(local_period)
        );
    }
}
