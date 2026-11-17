//! Live range estimator for the EV Vision dashboard.
//!
//! The Range-tab estimate is the starting value. Efficiency is calibrated after
//! 2 km or 2 percentage points of SoC consumption, whichever happens first.
//! Once calibrated, the estimate changes only after each additional 800 metres.
//! A rise in battery SoC starts a new cycle from the current Range-tab estimate.
//! Trip-counter resets (for example on lock/unlock) never reset the estimate.

#[derive(Debug, Clone)]
pub struct RangeEstimator {
    range_km: Option<f64>,
    trip_km: Option<f64>,
    total_distance_km: f64,
    fine_soc_pct: Option<f64>,
    baseline_distance_km: Option<f64>,
    baseline_soc_pct: Option<f64>,
    baseline_range_km: Option<f64>,
    calibrated: bool,
    last_update_distance_km: f64,
    predicted_km: Option<f64>,
}

impl RangeEstimator {
    pub const fn new() -> Self {
        Self {
            range_km: None,
            trip_km: None,
            total_distance_km: 0.0,
            fine_soc_pct: None,
            baseline_distance_km: None,
            baseline_soc_pct: None,
            baseline_range_km: None,
            calibrated: false,
            last_update_distance_km: 0.0,
            predicted_km: None,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn set_range(&mut self, value: f64) {
        self.range_km = valid_range(value);
        // Range-tab telemetry may update frequently. Never overwrite an active
        // prediction just because that source changed.
        self.initialize_if_needed();
    }

    pub fn set_trip(&mut self, value: f64) {
        if let Some(current) = valid_distance(value) {
            if let Some(previous) = self.trip_km {
                if current >= previous {
                    self.total_distance_km += current - previous;
                }
                // A falling trip counter is a trip reset. Re-anchor the counter
                // without changing accumulated distance or prediction.
            }
            self.trip_km = Some(current);
        }
        self.initialize_if_needed();
        self.refresh_for_distance();
    }

    pub fn set_fine_soc(&mut self, value: f64) {
        let next = valid_soc(value);
        if let (Some(previous), Some(current)) = (self.fine_soc_pct, next) {
            // A battery percentage increase is the reliable charging signal.
            // It starts a fresh prediction cycle even if the charger-status
            // telemetry or trip counter has not changed.
            if current > previous + 0.01 {
                self.start_new_charge_cycle();
            }
        }
        self.fine_soc_pct = next;
        self.initialize_if_needed();
        self.refresh_for_distance();
    }

    // Retained for existing telemetry callers. A charger-status flag alone does
    // not refresh the prediction; the battery percentage increase does.
    pub fn set_charging(&mut self, _charging: bool) {}

    pub fn predicted_range(&self) -> Option<f64> {
        self.predicted_km
    }

    pub fn is_calibrated(&self) -> bool {
        self.calibrated
    }

    fn start_new_charge_cycle(&mut self) {
        self.baseline_distance_km = Some(self.total_distance_km);
        self.baseline_soc_pct = self.fine_soc_pct;
        self.baseline_range_km = self.range_km;
        self.calibrated = false;
        self.last_update_distance_km = 0.0;
        // Use the vehicle's own Range-tab estimate as the new starting point.
        if let Some(range) = self.range_km {
            self.predicted_km = Some(range);
        }
    }

    fn initialize_if_needed(&mut self) {
        if self.baseline_distance_km.is_some()
            && self.baseline_soc_pct.is_some()
            && self.baseline_range_km.is_some()
        {
            return;
        }
        if let (Some(soc), Some(range)) = (self.fine_soc_pct, self.range_km) {
            self.baseline_distance_km = Some(self.total_distance_km);
            self.baseline_soc_pct = Some(soc);
            self.baseline_range_km = Some(range);
            self.predicted_km.get_or_insert(range);
        } else if self.predicted_km.is_none() {
            self.predicted_km = self.range_km;
        }
    }

    fn refresh_for_distance(&mut self) {
        let (Some(base_distance), Some(base_soc), Some(base_range), Some(soc)) = (
            self.baseline_distance_km,
            self.baseline_soc_pct,
            self.baseline_range_km,
            self.fine_soc_pct,
        ) else {
            return;
        };

        let distance = (self.total_distance_km - base_distance).max(0.0);
        let consumed = base_soc - soc;

        if !self.calibrated {
            if distance >= 2.0 || consumed >= 2.0 {
                if distance > 0.05 && consumed > 0.05 {
                    self.calibrated = true;
                    self.last_update_distance_km = distance;
                    self.recalculate(distance, consumed, soc);
                } else {
                    self.predicted_km = Some(base_range);
                }
            } else {
                self.predicted_km = Some(base_range);
            }
            return;
        }

        if distance - self.last_update_distance_km + 1e-9 >= 0.8 {
            self.last_update_distance_km +=
                ((distance - self.last_update_distance_km) / 0.8).floor() * 0.8;
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
    fn keeps_range_tab_estimate_until_calibration_threshold() {
        let mut r = seeded();
        r.set_trip(1.9);
        r.set_fine_soc(79.0);
        assert_eq!(r.predicted_range(), Some(120.0));
        assert!(!r.is_calibrated());
    }

    #[test]
    fn calibrates_at_two_kilometres() {
        let mut r = seeded();
        r.set_trip(2.0);
        r.set_fine_soc(79.5);
        assert!(r.is_calibrated());
        assert!((r.predicted_range().unwrap() - 318.0).abs() < 0.001);
    }

    #[test]
    fn calibrates_when_two_percent_is_consumed() {
        let mut r = seeded();
        r.set_trip(0.8);
        r.set_fine_soc(78.0);
        assert!(r.is_calibrated());
        assert!((r.predicted_range().unwrap() - 31.2).abs() < 0.001);
    }

    #[test]
    fn refreshes_only_after_each_additional_800_metres() {
        let mut r = seeded();
        r.set_trip(2.0);
        r.set_fine_soc(79.0);
        let first = r.predicted_range().unwrap();
        r.set_trip(2.79);
        r.set_fine_soc(78.5);
        assert_eq!(r.predicted_range(), Some(first));
        r.set_trip(2.8);
        assert!((r.predicted_range().unwrap() - (2.8 / 1.5 * 78.5)).abs() < 0.001);
    }

    #[test]
    fn trip_counter_reset_does_not_reset_prediction_or_distance_tracking() {
        let mut r = seeded();
        r.set_trip(2.0);
        r.set_fine_soc(79.0);
        let first = r.predicted_range();
        r.set_range(110.0);
        r.set_trip(0.0);
        assert!(r.is_calibrated());
        assert_eq!(r.predicted_range(), first);
        r.set_trip(0.8);
        assert_ne!(r.predicted_range(), first);
    }

    #[test]
    fn battery_percentage_increase_restarts_from_range_tab_estimate() {
        let mut r = seeded();
        r.set_trip(2.0);
        r.set_fine_soc(79.0);
        assert!(r.is_calibrated());
        r.set_range(105.0);
        r.set_fine_soc(80.0);
        assert!(!r.is_calibrated());
        assert_eq!(r.predicted_range(), Some(105.0));
        r.set_trip(2.8);
        r.set_fine_soc(79.5);
        assert!(!r.is_calibrated());
        assert_eq!(r.predicted_range(), Some(105.0));
    }

    #[test]
    fn charger_status_alone_does_not_refresh_prediction() {
        let mut r = seeded();
        r.set_trip(2.0);
        r.set_fine_soc(79.0);
        let prediction = r.predicted_range();
        r.set_charging(true);
        assert_eq!(r.predicted_range(), prediction);
        assert!(r.is_calibrated());
    }

    #[test]
    fn range_tab_updates_do_not_overwrite_live_prediction() {
        let mut r = seeded();
        r.set_trip(2.0);
        r.set_fine_soc(79.0);
        let prediction = r.predicted_range();
        r.set_range(99.0);
        assert_eq!(r.predicted_range(), prediction);
    }
}
