//! Live range estimator for the EV Vision dashboard.
//!
//! It holds the vehicle's own Range-tab estimate until either 2 km have been
//! travelled or 2 percentage points of SoC have been consumed. It then derives
//! km per percentage point from the observed ride and multiplies by remaining SoC.
//! Once calibrated, it only refreshes at each additional whole kilometre.

#[derive(Debug, Clone)]
pub struct RangeEstimator {
    range_km: Option<f64>,
    trip_km: Option<f64>,
    fine_soc_pct: Option<f64>,
    rounded_soc_pct: Option<f64>,
    baseline_trip_km: Option<f64>,
    baseline_soc_pct: Option<f64>,
    baseline_range_km: Option<f64>,
    charging: bool,
    calibrated: bool,
    last_update_km: u64,
    predicted_km: Option<f64>,
}

impl RangeEstimator {
    pub const fn new() -> Self {
        Self {
            range_km: None,
            trip_km: None,
            fine_soc_pct: None,
            rounded_soc_pct: None,
            baseline_trip_km: None,
            baseline_soc_pct: None,
            baseline_range_km: None,
            charging: false,
            calibrated: false,
            last_update_km: 0,
            predicted_km: None,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn set_range(&mut self, value: f64) {
        self.range_km = valid_range(value);
        self.refresh();
    }

    pub fn set_trip(&mut self, value: f64) {
        let next = valid_distance(value);
        if let (Some(previous), Some(current)) = (self.trip_km, next) {
            // The odometer-derived trip counter drops to zero on a fresh unlock.
            if current + 0.05 < previous {
                self.clear_calibration();
            }
        }
        self.trip_km = next;
        self.refresh();
    }

    pub fn set_fine_soc(&mut self, value: f64) {
        self.fine_soc_pct = valid_soc(value);
        self.refresh();
    }

    pub fn set_rounded_soc(&mut self, value: f64) {
        self.rounded_soc_pct = valid_soc(value);
        self.refresh();
    }

    pub fn set_charging(&mut self, charging: bool) {
        if charging && !self.charging {
            self.clear_calibration();
        }
        self.charging = charging;
        self.refresh();
    }

    pub fn predicted_range(&self) -> Option<f64> {
        self.predicted_km
    }

    pub fn is_calibrated(&self) -> bool {
        self.calibrated
    }

    fn current_soc(&self) -> Option<f64> {
        self.fine_soc_pct.or(self.rounded_soc_pct)
    }

    fn clear_calibration(&mut self) {
        self.baseline_trip_km = None;
        self.baseline_soc_pct = None;
        self.baseline_range_km = None;
        self.calibrated = false;
        self.last_update_km = 0;
        self.predicted_km = self.range_km;
    }

    fn refresh(&mut self) {
        if self.charging {
            self.predicted_km = self.range_km;
            self.calibrated = false;
            return;
        }

        if self.baseline_trip_km.is_none()
            || self.baseline_soc_pct.is_none()
            || self.baseline_range_km.is_none()
        {
            if let (Some(trip), Some(soc), Some(range)) =
                (self.trip_km, self.current_soc(), self.range_km)
            {
                self.baseline_trip_km = Some(trip);
                self.baseline_soc_pct = Some(soc);
                self.baseline_range_km = Some(range);
                self.predicted_km = Some(range);
                self.last_update_km = 0;
            } else {
                self.predicted_km = self.range_km;
                return;
            }
        }

        let (Some(trip), Some(soc), Some(base_trip), Some(base_soc)) = (
            self.trip_km,
            self.current_soc(),
            self.baseline_trip_km,
            self.baseline_soc_pct,
        ) else {
            // Hold the last valid prediction while one telemetry stream is absent.
            return;
        };

        let distance = (trip - base_trip).max(0.0);
        let consumed = base_soc - soc;

        if !self.calibrated {
            // Whichever threshold is reached first starts the efficiency estimate.
            if distance >= 2.0 || consumed >= 2.0 {
                if distance > 0.05 && consumed > 0.05 {
                    self.calibrated = true;
                    self.last_update_km = distance.floor() as u64;
                    self.recalculate(distance, consumed, soc);
                } else {
                    self.predicted_km = self.baseline_range_km;
                }
            } else {
                self.predicted_km = self.baseline_range_km;
            }
            return;
        }

        let completed_km = distance.floor() as u64;
        if completed_km > self.last_update_km {
            self.last_update_km = completed_km;
            if consumed > 0.05 && distance > 0.05 {
                self.recalculate(distance, consumed, soc);
            }
        }
    }

    fn recalculate(&mut self, distance: f64, consumed: f64, remaining_soc: f64) {
        let estimate = (distance / consumed) * remaining_soc;
        if estimate.is_finite() && estimate >= 0.0 {
            self.predicted_km = Some(estimate);
        }
    }
}

fn valid_range(value: f64) -> Option<f64> {
    (value.is_finite() && (0.0..=1000.0).contains(&value)).then_some(value)
}

fn valid_distance(value: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn valid_soc(value: f64) -> Option<f64> {
    (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::RangeEstimator;

    fn seeded() -> RangeEstimator {
        let mut r = RangeEstimator::new();
        r.set_range(120.0);
        r.set_trip(0.0);
        r.set_fine_soc(80.0);
        r
    }

    #[test]
    fn keeps_range_tab_estimate_until_a_threshold_is_reached() {
        let mut r = seeded();
        r.set_trip(1.9);
        r.set_fine_soc(79.0);
        assert_eq!(r.predicted_range(), Some(120.0));
        assert!(!r.is_calibrated());
    }

    #[test]
    fn calibrates_at_two_kilometres_even_if_soc_drop_is_smaller_than_two_percent() {
        let mut r = seeded();
        r.set_trip(2.0);
        r.set_fine_soc(79.5);
        assert!(r.is_calibrated());
        assert!((r.predicted_range().unwrap() - 318.0).abs() < 0.001);
    }

    #[test]
    fn calibrates_early_when_two_percent_is_consumed() {
        let mut r = seeded();
        r.set_trip(0.8);
        r.set_fine_soc(78.0);
        assert!(r.is_calibrated());
        assert!((r.predicted_range().unwrap() - 31.2).abs() < 0.001);
    }

    #[test]
    fn refreshes_only_after_each_additional_whole_kilometre() {
        let mut r = seeded();
        r.set_trip(2.0);
        r.set_fine_soc(79.0);
        let first = r.predicted_range().unwrap();
        r.set_fine_soc(78.5);
        assert_eq!(r.predicted_range(), Some(first));
        r.set_trip(3.0);
        assert!((r.predicted_range().unwrap() - 157.0).abs() < 0.001);
    }

    #[test]
    fn trip_reset_restarts_calibration_from_the_current_range_and_soc() {
        let mut r = seeded();
        r.set_trip(2.0);
        r.set_fine_soc(79.0);
        assert!(r.is_calibrated());
        r.set_range(110.0);
        r.set_trip(0.0);
        assert!(!r.is_calibrated());
        assert_eq!(r.predicted_range(), Some(110.0));
    }

    #[test]
    fn charging_displays_the_vehicle_estimate_and_restarts_calibration() {
        let mut r = seeded();
        r.set_trip(2.0);
        r.set_fine_soc(79.0);
        assert!(r.is_calibrated());
        r.set_range(105.0);
        r.set_charging(true);
        assert!(!r.is_calibrated());
        assert_eq!(r.predicted_range(), Some(105.0));
    }
}
