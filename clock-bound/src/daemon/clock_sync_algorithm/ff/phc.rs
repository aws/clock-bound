//! The PHC Feed-forward time synchronization algorithm

use std::num::NonZeroUsize;

use super::event_buffer;
use super::{CalculateThetaOutput, LocalPeriodAndError, UncorrectedClock};
use crate::daemon::{
    clock_parameters::ClockParameters,
    clock_sync_algorithm::ff::event_buffer::FeedError,
    clock_sync_algorithm::ring_buffer::Quarter,
    event::{self, TscRtt},
    time::{
        Clock, Duration, TscDiff,
        clocks::MonotonicCoarse,
        tsc::{Period, Skew},
    },
};

/// Feed forward time synchronization algorithm for a single PHC source
#[derive(Debug, Clone, PartialEq)]
pub struct Phc {
    /// Events within 100 seconds.
    ///
    /// Used for calculation of `period_local` and `theta`
    local: event_buffer::Local<event::Phc>,
    /// Staging data over the last 1024 seconds.
    ///
    /// This data is used solely to stage data before feeding
    /// into the estimate buffer
    staging: event_buffer::Local<event::Phc>,
    /// Best RTT values of each SKM over the last week
    estimate: event_buffer::Estimate<event::Phc>,
    /// Current calculation of [`ClockParameters`]
    clock_parameters: Option<ClockParameters>,
    /// Current uncorrected clock
    uncorrected_clock: Option<UncorrectedClock>,
    /// Max dispersion growth
    ///
    /// Used to compare clock error bounds with older samples while
    /// taking into account CEB growth from dispersion
    max_dispersion: Skew,
}

impl Phc {
    /// Create a new feed forward time synchronization algorithm
    ///
    /// `local_capacity` should be the number of data-points to span an SKM window.
    /// For example, if the source is expected to sample once every second, the `local_capacity`
    /// should have a max value of 1024.
    ///
    /// # Panics
    /// Panics if poll duration is zero or greater than or equal to 512 seconds
    pub fn new(poll_period: Duration, max_dispersion: Skew) -> Self {
        const WINDOW: Duration = Duration::from_secs(100);
        assert!(poll_period > Duration::ZERO, "poll period must be positive");
        assert!(
            poll_period < WINDOW / 2,
            "Must be able to get at least 2 samples in local buffer"
        );
        let local_capacity = WINDOW.get() / poll_period.get();

        // unwrap: input is bound and numerator is never 0
        let local_capacity = NonZeroUsize::new(local_capacity.try_into().unwrap()).unwrap();

        // unwrap: input is bound and numerator is never 0
        let staging_capacity = event_buffer::Local::<Phc>::SKM_WINDOW.get() / poll_period.get();
        let staging_capacity = NonZeroUsize::new(staging_capacity.try_into().unwrap()).unwrap();

        Self {
            local: event_buffer::Local::with_window(local_capacity, WINDOW),
            staging: event_buffer::Local::new(staging_capacity),
            estimate: event_buffer::Estimate::new(),
            clock_parameters: None,
            uncorrected_clock: None,
            max_dispersion,
        }
    }

    /// Feed an event into this algorithm
    ///
    /// Returns [`Some`] if the event has improved this source's [`ClockParameters`].
    pub fn feed(&mut self, event: event::Phc) -> Option<&ClockParameters> {
        // FIXME: take a MONOTONIC_COARSE timestamp *before* computing the clock error bound.
        //
        // The only use of this timestamp is to support and maintain the behavior for clients built
        // against ClockBound 2.0. These clients grow the CEB by calculating the time elapsed
        // between the instant the ClockParameters were computed, and the instant they read the
        // system clock. This needs to be a bit pessimistic, hence the as_of_monotonic timestamp is
        // taken *before* computing the clock error bound.
        //
        // This could be eliminated once we decide to stop supporting ClockBound 2.0 clients.
        let as_of_monotonic = MonotonicCoarse.get_time();

        let tsc_midpoint = event.tsc_midpoint();

        // First update the internal local (current SKM) and estimate (long term)
        // sample buffers
        let within_threshold = self
            .feed_internal_buffers(event)
            .inspect_err(|error_msg| match error_msg {
                FeedError::Old { .. } => {
                    tracing::warn!(?error_msg);
                }
            })
            .ok()?; // early exit if there was an error with the sample

        if !within_threshold {
            // At this point, if the input does not meet our expectations on the rtt threshold,
            // there is no more processing to do. The end calculation will not be more accurate than
            // the previous value (if we have one)
            tracing::trace!("Early exit. Event not within threshold.");
            return None;
        }

        // Functionality from this point will fill out the equation
        // `C(t) = TSC(t) × p^ + K − θ^(t)` where:
        // - `C(t)` is the absolute time. Corrected. This is effectively the output of the clock sync algorithm
        // - `TSC(t)` is the tsc reading at a time
        // - `p^` is the estimation of the clock period
        // - `K` is the "epoch" (the uncorrected time at `TSC(0)`)
        // - `θ^(t)` is the time correction

        // Calculate uncorrected clock, aka `p^` and `K`
        self.uncorrected_clock = Self::calculate_uncorrected_clock(&self.local, &self.estimate);

        // Then calculate the local period using just the local event buffer
        //
        // This value is not used in the above equation, but IS reported in the final clock parameters
        let Some(local_period) = Self::calculate_local_period_and_error(&self.local) else {
            tracing::debug!("Early exit. Calculate local period returned none.");
            return None;
        };

        // expect because not having an uncorrected clock is a bug at this point
        //
        // If we are able to calculate a local period, then uncorrected clock must be available
        #[expect(clippy::missing_panics_doc, reason = "comment above")]
        let uncorrected_clock = self
            .uncorrected_clock
            .expect("No uncorrected period but we have local period");

        // Calculate `θ^(t)`
        let CalculateThetaOutput {
            theta,
            clock_error_bound,
        } = Self::calculate_theta(&self.local, local_period.period_local, uncorrected_clock);

        // Calculate `C(t)`
        //
        // `uncorrected_clock.time_at(tsc)` is a function that calculates `TSC(t) × p^ + K`
        // So this expands to `time = TSC(t) × p^ + K - θ^(t)`
        //
        // uses `tsc_midpoint` as that is the value that was used during the `Self::calculate_theta` fn
        let time = uncorrected_clock.time_at(tsc_midpoint) - theta;

        // Emit internal calculations for after-the-fact analysis (replay only,
        // no-op in daemon builds; see the algo_analysis module)

        #[cfg(feature = "algorithm")]
        crate::daemon::algo_analysis::emit_theta_calculation(
            "phc",
            theta,
            clock_error_bound,
            uncorrected_clock,
            &local_period,
            tsc_midpoint,
            time,
        );

        let clock_parameters = ClockParameters {
            tsc_count: tsc_midpoint,
            time,
            clock_error_bound,
            period: local_period.period_local,
            period_max_error: local_period.error,
            as_of_monotonic,
        };

        match &mut self.clock_parameters {
            None => {
                // This is the first time we have calculated clock parameters. Set it.
                tracing::debug!(?clock_parameters, "Clock Parameters initialized.");
                self.clock_parameters = Some(clock_parameters);
                self.clock_parameters.as_ref()
            }
            Some(current_clock_parameters)
                if clock_parameters
                    .more_accurate_than(current_clock_parameters, self.max_dispersion) =>
            {
                // We currently have clock_parameters, and the new value is more accurate. Replace it.
                tracing::debug!(?clock_parameters, "Clock Parameters updated.");
                *current_clock_parameters = clock_parameters;
                self.clock_parameters.as_ref()
            }
            Some(current_clock_parameters) => {
                // We currently have clock_parameters, and the new value is NOT more accurate. Ignore and return None.
                tracing::debug!(new_clock_parameters = ?clock_parameters, ?current_clock_parameters, "Clock Parameters not updated.");
                None
            }
        }
    }

    /// Get the current [`ClockParameters`]
    pub fn clock_parameters(&self) -> Option<&ClockParameters> {
        self.clock_parameters.as_ref()
    }

    /// Feed the internal buffers with a new event
    ///
    /// Returns `Ok(true)` if the value is within the local buffer's min rtt threshold
    ///
    /// Returns `Ok(false)` if the inserted value exceeds the local buffer's min rtt threshold
    ///
    /// This updates the local buffer with the event. If a `period` has been calculated already,
    /// then we can use this value to update the `estimate` buffer as well.
    fn feed_internal_buffers(&mut self, event: event::Phc) -> Result<bool, FeedError<event::Phc>> {
        let event_rtt = event.rtt();
        self.local.feed(event.clone())?;
        self.staging.feed(event)?;

        if let Some(uc) = self.uncorrected_clock {
            self.local.expunge_old_events(uc.p_estimate);
            self.staging.expunge_old_events(uc.p_estimate);
            self.estimate.feed(&self.staging, uc.p_estimate);
        }
        // unwrap okay. Above line ensures there is at least one value in the buffer
        let min_rtt_event = self.local.as_ref().min_rtt().unwrap();
        let within_threshold =
            event_rtt <= (min_rtt_event.rtt() * self.local.rtt_threshold_multiplier());
        Ok(within_threshold)
    }

    /// Handle a disruption event
    ///
    /// Clears all event buffers and prior-calculations.
    pub fn handle_disruption(&mut self) {
        // Destructure pattern makes handling new fields mandatory
        let Self {
            local,
            staging,
            estimate,
            clock_parameters,
            uncorrected_clock,
            max_dispersion: _, // value currently does not change
        } = self;

        local.handle_disruption();
        staging.handle_disruption();
        estimate.handle_disruption();
        *clock_parameters = None;
        *uncorrected_clock = None;
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
        local: &event_buffer::Local<event::Phc>,
        estimate: &event_buffer::Estimate<event::Phc>,
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

        // calculate using the backward offset (counter_post and reference clock time)
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
        // A complex strategy we could take on would be to calculate error values on slopes and directly compare
        // these values in search. That kind of approach is TBD.
        //
        // TODO explore this a bit more.
        let p_estimate = oldest.calculate_period(newest);

        let k = p_estimate * TscDiff::new(newest.counter_post().get());
        let k = newest.data().time - k;

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
    fn calculate_local_period_and_error(
        local: &event_buffer::Local<event::Phc>,
    ) -> Option<LocalPeriodAndError> {
        if local.as_ref().len() < 2 {
            return None;
        }
        // unwrap okay, length of 2 means both quarters and min check will succeed
        let old = local.as_ref().min_rtt_in_quarter(Quarter::Oldest).unwrap();
        let new = local.as_ref().min_rtt_in_quarter(Quarter::Newest).unwrap();

        let local_period_and_error = old.calculate_period_with_error(new);

        tracing::debug!(
            ?old,
            ?new,
            ?local_period_and_error,
            "Calculated local period and error"
        );
        Some(local_period_and_error)
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
    /// Calculation requires calculating
    /// ```text
    ///           ∑{ wᵢ × (offsetᵢ + skew × p̂ × (TSCₚₒₛₜ,ₗₐₛₜ − TSCₚₒₛₜ,ᵢ))
    /// θ̂(tₗₐₛₜ) = ---------------------------------------------------------
    ///                                  ∑{wᵢ}
    /// ```
    ///
    /// where:
    /// ```text
    /// wᵢ = exp(−√(Eᵢ/E))
    /// and
    /// Eᵢ = RTTᵢ − min(RTT)
    /// ```
    ///  
    ///
    /// # Panics
    /// Panics if `Local` is empty
    #[expect(
        clippy::cast_precision_loss,
        reason = "exp and weight require floats. Values are small enough to not lose precision"
    )]
    fn calculate_theta(
        local: &event_buffer::Local<event::Phc>,
        period_local: Period,
        uncorrected_clock: UncorrectedClock,
    ) -> CalculateThetaOutput {
        // Feed-forward time synchronization algorithm's error normalization factor.
        // This constant is used to penalize the feed_forward_samples that have a
        // slower reference clock read duration.
        const ERROR_NORMALIZATION_FACTOR: f64 = 1e5;

        assert!(!local.is_empty());

        let now_post = local.as_ref().head().unwrap().counter_post();
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
                * ((now_post - event.counter_post()).get() as f64);
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

#[cfg(test)]
mod tests {
    use crate::daemon::{
        clock_sync_algorithm::{
            RingBuffer,
            ff::event_buffer::{Estimate, Local},
        },
        event::PhcData,
        time::{Duration, Instant, TscCount},
    };

    use super::*;

    #[test]
    fn empty_buffers() {
        let local = Local::new(NonZeroUsize::new(1).unwrap());
        let estimate = Estimate::new();
        let result = Phc::calculate_uncorrected_clock(&local, &estimate);
        assert!(result.is_none());
    }

    #[test]
    fn calculate_local_period_returns_none() {
        // return none if local has < 2 events

        let mut local = event_buffer::Local::new(NonZeroUsize::new(2).unwrap());
        let result = Phc::calculate_local_period_and_error(&local);
        assert!(result.is_none());

        local
            .feed(
                event::Phc::builder()
                    .counter_pre(TscCount::new(0))
                    .counter_post(TscCount::new(1_000))
                    .data(PhcData {
                        time: Instant::from_days(1) + Duration::from_nanos(2500),
                        clock_error_bound: Duration::from_nanos(1500),
                    })
                    .build()
                    .unwrap(),
            )
            .unwrap();

        let result = Phc::calculate_local_period_and_error(&local);
        assert!(result.is_none());
    }

    #[test]
    fn calculate_local_period() {
        let mut local = event_buffer::Local::new(NonZeroUsize::new(2).unwrap());

        let old_event = event::Phc::builder()
            .counter_pre(TscCount::new(0))
            .counter_post(TscCount::new(1_000_000))
            .data(PhcData {
                time: Instant::from_days(1) + Duration::from_nanos(2500),
                clock_error_bound: Duration::from_nanos(6350),
            })
            .build()
            .unwrap();

        // new event is 100 seconds in the future with tsc at 1GHz
        let new_event = event::Phc::builder()
            .counter_pre(TscCount::new(100_000_000_000))
            .counter_post(TscCount::new(100_001_000_000))
            .data(PhcData {
                time: Instant::from_days(1) + Duration::from_secs(100) + Duration::from_nanos(2500),
                clock_error_bound: Duration::from_nanos(6350),
            })
            .build()
            .unwrap();
        local.feed(old_event).unwrap();
        local.feed(new_event).unwrap();

        let result = Phc::calculate_local_period_and_error(&local).unwrap();
        assert_eq!(result.period_local, Period::from_seconds(1.0e-9));
    }

    #[test]
    fn calculate_uncorrected_local_has_single_value() {
        let mut local = Local::new(NonZeroUsize::new(1).unwrap());
        let event = event::Phc::builder()
            .counter_pre(TscCount::new(100))
            .counter_post(TscCount::new(110))
            .data(event::PhcData {
                time: Instant::from_days(1) + Duration::from_micros(5),
                clock_error_bound: Duration::from_micros(32),
            })
            .build()
            .unwrap();

        local.feed(event.clone()).unwrap();
        let estimate = Estimate::new();

        // estimate is empty, local has a single value. Cannot calculate a period with a single datapoint
        let result = Phc::calculate_uncorrected_clock(&local, &estimate);
        assert!(result.is_none());
    }

    #[test]
    fn calculate_uncorrected_local_has_two_values() {
        // Create 2 points that are 1 second apart and the TSC is roughly 1 GHz
        let mut local = Local::new(NonZeroUsize::new(2).unwrap());
        let ref_time = Instant::from_days(1) + Duration::from_nanos(500);
        let event1 = event::Phc::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_001_000))
            .data(event::PhcData {
                time: ref_time,
                clock_error_bound: Duration::from_micros(32),
            })
            .build()
            .unwrap();

        let event2 = event::Phc::builder()
            .counter_pre(TscCount::new(2_000_000_000))
            .counter_post(TscCount::new(2_000_001_000))
            .data(event::PhcData {
                time: Instant::from_days(1) + Duration::from_secs(1) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(32),
            })
            .build()
            .unwrap();

        local.feed(event1.clone()).unwrap();
        local.feed(event2.clone()).unwrap();
        let estimate = Estimate::new();

        let result = Phc::calculate_uncorrected_clock(&local, &estimate).unwrap();

        approx::assert_abs_diff_eq!(result.p_estimate.get(), 1e-9);
        let expected = ref_time - Duration::from_secs(1) - Duration::from_nanos(1000); // account for counter_post time of 1_000_001_000 at 1GHz period
        assert_eq!(result.k, expected);
    }

    #[test]
    fn calculate_uncorrected_estimate_has_values() {
        // Create 2 points that are 1 second apart and the TSC is roughly 1 GHz
        let mut local = Local::new(NonZeroUsize::new(2).unwrap());
        let local_event = event::Phc::builder()
            .counter_pre(TscCount::new(2_000_000_000_000))
            .counter_post(TscCount::new(2_000_000_001_000))
            .data(event::PhcData {
                time: Instant::from_days(1) + Duration::from_secs(1999) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(32),
            })
            .build()
            .unwrap();

        local.feed(local_event).unwrap();

        let estimate_event_1 = event::Phc::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_001_000))
            .data(event::PhcData {
                time: Instant::from_days(1) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(32),
            })
            .build()
            .unwrap();

        // larger RTT
        let estimate_event_2 = event::Phc::builder()
            .counter_pre(TscCount::new(1_000_000_000_000))
            .counter_post(TscCount::new(1_000_000_002_000))
            .data(event::PhcData {
                time: Instant::from_days(1) + Duration::from_secs(1000) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(32),
            })
            .build()
            .unwrap();

        let mut estimate_inner = RingBuffer::new(NonZeroUsize::new(5).unwrap());
        estimate_inner.push(estimate_event_1);
        estimate_inner.push(estimate_event_2);

        let estimate = Estimate::builder().inner(estimate_inner).build();

        let result = Phc::calculate_uncorrected_clock(&local, &estimate).unwrap();

        approx::assert_abs_diff_eq!(result.p_estimate.get(), 1e-9);
        assert_eq!(
            result.k,
            Instant::from_days(1) + Duration::from_nanos(500)
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
        let event1 = event::Phc::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_001_000))
            .data(event::PhcData {
                time: Instant::from_days(1) + Duration::from_secs(1) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(32),
            })
            .build()
            .unwrap();

        let event2 = event::Phc::builder()
            .counter_pre(TscCount::new(2_000_000_000))
            .counter_post(TscCount::new(2_000_001_000))
            .data(event::PhcData {
                time: Instant::from_days(1) + Duration::from_secs(2) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(32),
            })
            .build()
            .unwrap();

        local.feed(event1.clone()).unwrap();
        local.feed(event2.clone()).unwrap();

        let CalculateThetaOutput {
            theta,
            clock_error_bound,
        } = Phc::calculate_theta(&local, local_period, uncorrected_clock);

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
        let event1 = event::Phc::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_001_000))
            .data(event::PhcData {
                time: Instant::from_days(1) + Duration::from_secs(1) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(42),
            })
            .build()
            .unwrap();

        let event2 = event::Phc::builder()
            .counter_pre(TscCount::new(2_000_000_000))
            .counter_post(TscCount::new(2_000_001_000))
            .data(event::PhcData {
                time: Instant::from_days(1) + Duration::from_secs(2) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(32),
            })
            .build()
            .unwrap();

        local.feed(event1.clone()).unwrap();
        local.feed(event2.clone()).unwrap();

        let CalculateThetaOutput {
            theta,
            clock_error_bound,
        } = Phc::calculate_theta(&local, local_period, uncorrected_clock);

        assert_eq!(theta, Duration::from_micros(-5));
        assert_eq!(
            clock_error_bound,
            event1.calculate_clock_error_bound(local_period)
        );
    }

    #[test]
    fn feed_two_events() {
        let mut ff = Phc::new(Duration::from_secs(20), Skew::from_ppm(15.0));

        let event1 = event::Phc::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_001_000))
            .data(event::PhcData {
                time: Instant::from_days(1) + Duration::from_secs(1) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(42),
            })
            .build()
            .unwrap();

        let event2 = event::Phc::builder()
            .counter_pre(TscCount::new(2_000_000_000))
            .counter_post(TscCount::new(2_000_001_000))
            .data(event::PhcData {
                time: Instant::from_days(1) + Duration::from_secs(2) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(32),
            })
            .build()
            .unwrap();

        let result = ff.feed(event1.clone());
        assert!(result.is_none());

        let result = ff.feed(event2.clone());
        let clock_params = result.unwrap();

        assert_eq!(clock_params.time, event2.data().time);

        // events are symmetric RTT and at 1GHz
        approx::assert_abs_diff_eq!(clock_params.period.get(), 1e-9);

        // clock_error_bound should be max value
        let expected_ceb = event1.calculate_clock_error_bound(clock_params.period);
        assert_eq!(clock_params.clock_error_bound, expected_ceb);
    }
}
