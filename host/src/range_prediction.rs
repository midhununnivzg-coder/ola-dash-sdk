//! Live range estimator for the EV Vision dashboard.
//!
//! Uses the vehicle odometer (not the resettable trip meter) so lock/unlock and
//! trip resets cannot interrupt distance tracking. After the initial 2 km / 2%
//! calibration, refreshes after each additional 800 m. A rise in SoC starts a
//! new cycle from the current Range-tab estimate.

#[derive(Debug, Clone)]
pub struct RangeEstimator {
    range_km: Option<f64>,
    odometer_km: Option<f64>,
    fine_soc_pct: Option<f64>,
    battery_pct: Option<f64>,
    awaiting_charge_soc: bool,
    baseline_odometer_km: Option<f64>,
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
            odometer_km: None,
            fine_soc_pct: None,
            battery_pct: None,
            awaiting_charge_soc: false,
            baseline_odometer_km: None,
            baseline_soc_pct: None,
            baseline_range_km: None,
            calibrated: false,
            last_update_distance_km: 0.0,
            predicted_km: None,
        }
    }

    pub fn reset(&mut self) { *self = Self::new(); }

    pub fn set_range(&mut self, value: f64) {
        self.range_km = valid_range(value);
        // Range-tab updates never overwrite an active live prediction.
        self.initialize_if_needed();
    }

    /// The odometer is monotonic across trip resets and lock/unlock cycles.
    pub fn set_odometer(&mut self, value: f64) {
        if let Some(current) = valid_distance(value) {
            if self.odometer_km.map_or(true, |previous| current >= previous) {
                self.odometer_km = Some(current);
            }
        }
        self.initialize_if_needed();
        self.refresh_for_distance();
    }

    /// Whole-number dashboard battery percentage is the sole charge/reset trigger.
    pub fn set_battery_pct(&mut self, value: f64) {
        let next = valid_soc(value);
        if let (Some(previous), Some(current)) = (self.battery_pct, next) {
            if current > previous {
                self.start_new_charge_cycle();
                self.awaiting_charge_soc = true;
            }
        }
        self.battery_pct = next;
    }

    pub fn set_fine_soc(&mut self, value: f64) {
        self.fine_soc_pct = valid_soc(value);
        // Anchor to the fine SoC sample associated with the whole-percent rise.
        if self.awaiting_charge_soc {
            self.baseline_odometer_km = self.odometer_km;
            self.baseline_soc_pct = self.fine_soc_pct;
            self.baseline_range_km = self.range_km;
            self.last_update_distance_km = 0.0;
            self.awaiting_charge_soc = false;
        }
        self.initialize_if_needed();
        self.refresh_for_distance();
    }

    // A charger-status transition alone must never reset the estimate.
    pub fn set_charging(&mut self, _charging: bool) {}

    pub fn predicted_range(&self) -> Option<f64> { self.predicted_km }
    pub fn is_calibrated(&self) -> bool { self.calibrated }

    fn start_new_charge_cycle(&mut self) {
        self.baseline_odometer_km = self.odometer_km;
        self.baseline_soc_pct = self.fine_soc_pct;
        self.baseline_range_km = self.range_km;
        self.calibrated = false;
        self.last_update_distance_km = 0.0;
        if let Some(range) = self.range_km {
            self.predicted_km = Some(range);
        }
    }

    fn initialize_if_needed(&mut self) {
        if self.baseline_odometer_km.is_some()
            && self.baseline_soc_pct.is_some()
            && self.baseline_range_km.is_some() { return; }

        if let (Some(odo), Some(soc), Some(range)) =
            (self.odometer_km, self.fine_soc_pct, self.range_km) {
            self.baseline_odometer_km = Some(odo);
            self.baseline_soc_pct = Some(soc);
            self.baseline_range_km = Some(range);
            self.predicted_km.get_or_insert(range);
        } else if self.predicted_km.is_none() {
            self.predicted_km = self.range_km;
        }
    }

    fn refresh_for_distance(&mut self) {
        let (Some(base_odo), Some(base_soc), Some(base_range), Some(odo), Some(soc)) = (
            self.baseline_odometer_km, self.baseline_soc_pct, self.baseline_range_km,
            self.odometer_km, self.fine_soc_pct,
        ) else { return; };

        let distance = (odo - base_odo).max(0.0);
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

        // Advance in exact 0.8 km steps. Recalculate from the full current
        // ride distance/SoC consumption, but keep the displayed value unchanged
        // between those thresholds.
        let steps = ((distance - self.last_update_distance_km + 1e-9) / 0.8).floor();
        if steps >= 1.0 {
            self.last_update_distance_km += steps * 0.8;
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

fn valid_range(v: f64) -> Option<f64> {
    (v.is_finite() && (0.0..=1000.0).contains(&v)).then_some(v)
}
fn valid_distance(v: f64) -> Option<f64> {
    (v.is_finite() && v >= 0.0).then_some(v)
}
fn valid_soc(v: f64) -> Option<f64> {
    (v.is_finite() && (0.0..=100.0).contains(&v)).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::RangeEstimator;

    fn seeded() -> RangeEstimator {
        let mut r = RangeEstimator::new();
        r.set_range(120.0);
        r.set_odometer(1000.0);
        r.set_fine_soc(80.0);
        r.set_battery_pct(80.0);
        r
    }

    #[test]
    fn keeps_range_tab_estimate_until_initial_calibration() {
        let mut r = seeded();
        r.set_odometer(1001.9);
        r.set_fine_soc(79.0);
        assert_eq!(r.predicted_range(), Some(120.0));
        assert!(!r.is_calibrated());
    }

    #[test]
    fn calibrates_at_two_kilometres() {
        let mut r = seeded();
        r.set_odometer(1002.0);
        r.set_fine_soc(79.5);
        assert!(r.is_calibrated());
        assert!((r.predicted_range().unwrap() - 322.0).abs() < 0.001);
    }

    #[test]
    fn calibrates_when_two_percent_is_consumed() {
        let mut r = seeded();
        r.set_odometer(1000.8);
        r.set_fine_soc(78.0);
        assert!(r.is_calibrated());
        assert!((r.predicted_range().unwrap() - 31.2).abs() < 0.001);
    }

    #[test]
    fn refreshes_after_each_additional_800_metres_and_holds_between_steps() {
        let mut r = seeded();
        r.set_odometer(1002.0);
        r.set_fine_soc(79.0);
        let first = r.predicted_range().unwrap();
        r.set_odometer(1002.79);
        r.set_fine_soc(78.5);
        assert_eq!(r.predicted_range(), Some(first));
        r.set_odometer(1002.8);
        assert!((r.predicted_range().unwrap() - (2.8 / 1.5 * 78.5)).abs() < 0.001);
    }

    #[test]
    fn trip_resets_and_lock_unlock_do_not_affect_odometer_prediction() {
        let mut r = seeded();
        r.set_odometer(1002.0);
        r.set_fine_soc(79.0);
        let first = r.predicted_range();
        r.set_odometer(1002.8);
        assert_ne!(r.predicted_range(), first);
    }

    #[test]
    fn battery_percent_increase_restarts_from_range_tab_estimate() {
        let mut r = seeded();
        r.set_odometer(1002.0);
        r.set_fine_soc(79.0);
        assert!(r.is_calibrated());
        r.set_range(105.0);
        r.set_battery_pct(81.0);
        r.set_fine_soc(81.0);
        assert!(!r.is_calibrated());
        assert_eq!(r.predicted_range(), Some(105.0));
    }

    #[test]
    fn after_battery_increase_new_cycle_calibrates_from_the_higher_soc() {
        let mut r = seeded();
        r.set_odometer(1002.0);
        r.set_fine_soc(79.0);
        r.set_range(105.0);
        r.set_battery_pct(81.0);
        r.set_fine_soc(81.0);
        r.set_odometer(1004.0);
        r.set_fine_soc(80.5);
        assert!(r.is_calibrated());
        assert!((r.predicted_range().unwrap() - 318.0).abs() < 0.001);
    }

    #[test]
    fn charger_status_alone_does_not_refresh_prediction() {
        let mut r = seeded();
        r.set_odometer(1002.0);
        r.set_fine_soc(79.0);
        let prediction = r.predicted_range();
        r.set_charging(true);
        assert_eq!(r.predicted_range(), prediction);
        assert!(r.is_calibrated());
    }

    #[test]
    fn range_tab_updates_do_not_overwrite_live_prediction() {
        let mut r = seeded();
        r.set_odometer(1002.0);
        r.set_fine_soc(79.0);
        let prediction = r.predicted_range();
        r.set_range(99.0);
        assert_eq!(r.predicted_range(), prediction);
    }
}
