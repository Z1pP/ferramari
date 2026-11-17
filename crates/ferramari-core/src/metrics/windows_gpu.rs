//! Windows GPU utilization from the PDH "GPU Engine" counter set.
//!
//! `\GPU Engine(*)\Utilization Percentage` is the same source Task Manager
//! uses. It reports one rate per process and engine instance, for example
//! `pid_1234_luid_0x..._phys_0_eng_0_engtype_3D`. Dropping the per-process
//! `pid_..._` prefix lets the instances of one engine be summed, so two
//! processes sharing the 3D engine are not under-counted. The busiest engine
//! is the single number the Performance page shows.
//!
//! PDH computes rates from two collections, so the first call returns
//! `Pending`; later calls return the busiest engine percentage. A machine or
//! driver without the counter set reports `Unsupported`, never zero.

use std::collections::HashMap;

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA,
};

use super::Metric;

const COUNTER_PATH: &str = r"\GPU Engine(*)\Utilization Percentage";

#[derive(Debug, Default)]
pub struct Sampler {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
    primed: bool,
    failed: bool,
}

impl Drop for Sampler {
    fn drop(&mut self) {
        if !self.query.is_invalid() {
            unsafe {
                let _ = PdhCloseQuery(self.query);
            }
        }
    }
}

impl Sampler {
    pub fn sample(&mut self) -> Metric<f64> {
        if self.failed {
            return Metric::Unsupported;
        }
        if self.query.is_invalid() && !self.open() {
            return Metric::Unsupported;
        }
        if unsafe { PdhCollectQueryData(self.query) } != ERROR_SUCCESS.0 {
            return Metric::Unavailable;
        }
        if !self.primed {
            // A rate has no interval until the query has been collected twice.
            self.primed = true;
            return Metric::Pending;
        }
        self.collect()
    }

    fn open(&mut self) -> bool {
        let mut query = PDH_HQUERY::default();
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != ERROR_SUCCESS.0 {
            self.failed = true;
            return false;
        }
        let path: Vec<u16> = COUNTER_PATH.encode_utf16().chain([0]).collect();
        let mut counter = PDH_HCOUNTER::default();
        let status =
            unsafe { PdhAddEnglishCounterW(query, PCWSTR(path.as_ptr()), 0, &mut counter) };
        if status != ERROR_SUCCESS.0 {
            unsafe {
                let _ = PdhCloseQuery(query);
            }
            self.failed = true;
            return false;
        }
        self.query = query;
        self.counter = counter;
        true
    }

    fn collect(&self) -> Metric<f64> {
        let mut size = 0u32;
        let mut count = 0u32;
        let status = unsafe {
            PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
        };
        if status != PDH_MORE_DATA && status != ERROR_SUCCESS.0 {
            return Metric::Unavailable;
        }
        if size == 0 || count == 0 {
            return Metric::Unsupported;
        }

        // `size` is bytes including the instance-name strings. The array is
        // sized by element count; the API writes the names into the same
        // allocation and points `szName` at them.
        let capacity =
            (size as usize).div_ceil(std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>()) + 1;
        let mut items = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); capacity];
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                Some(items.as_mut_ptr()),
            )
        };
        if status != ERROR_SUCCESS.0 {
            return Metric::Unavailable;
        }

        let mut engines: HashMap<String, f64> = HashMap::new();
        for item in items.iter().take(count as usize) {
            if item.FmtValue.CStatus > 1 {
                continue;
            }
            let value = unsafe { item.FmtValue.Anonymous.doubleValue };
            if !value.is_finite() || value < 0.0 {
                continue;
            }
            let name = unsafe { item.szName.to_string() }.unwrap_or_default();
            *engines.entry(engine_key(&name).to_string()).or_default() += value;
        }
        if engines.is_empty() {
            return Metric::Unsupported;
        }
        Metric::Value(busiest_engine(&engines).clamp(0.0, 100.0))
    }
}

/// Drop the `pid_<digits>_` prefix so every process contributes to the same
/// engine instance.
fn engine_key(instance: &str) -> &str {
    instance
        .strip_prefix("pid_")
        .and_then(|rest| rest.split_once('_'))
        .map_or(instance, |(_, rest)| rest)
}

fn busiest_engine(engines: &HashMap<String, f64>) -> f64 {
    engines.values().copied().fold(0.0f64, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instances_of_one_engine_sum_and_the_busiest_engine_wins() {
        let mut engines = HashMap::new();
        *engines
            .entry(engine_key("pid_10_luid_0x0_0x1_phys_0_eng_0_engtype_3D").to_string())
            .or_default() += 20.0;
        *engines
            .entry(engine_key("pid_20_luid_0x0_0x1_phys_0_eng_0_engtype_3D").to_string())
            .or_default() += 35.0;
        *engines
            .entry(engine_key("pid_10_luid_0x0_0x1_phys_0_eng_0_engtype_Copy").to_string())
            .or_default() += 10.0;
        assert_eq!(busiest_engine(&engines), 55.0);
    }

    #[test]
    fn the_process_prefix_is_removed_but_other_names_survive() {
        assert_eq!(
            engine_key("pid_1234_luid_0x0_0x1_phys_0_eng_0_engtype_3D"),
            "luid_0x0_0x1_phys_0_eng_0_engtype_3D"
        );
        assert_eq!(engine_key("odd-instance"), "odd-instance");
    }

    #[test]
    fn the_counter_returns_a_percentage_or_says_why_not() {
        let mut sampler = Sampler::default();
        let first = sampler.sample();
        assert!(matches!(first, Metric::Pending | Metric::Unsupported));
        let sample = sampler.sample();
        println!("GPU sample: {sample:?}");
        match sample {
            Metric::Value(value) => assert!((0.0..=100.0).contains(&value)),
            Metric::Unsupported | Metric::Unavailable | Metric::Pending => {}
        }
    }
}
