//! Measures retained allocations from repeated CoreAudio device-name queries.
use cpal::traits::{DeviceTrait, HostTrait};

fn live_bytes() -> usize {
    let mut stats: libc::malloc_statistics_t = unsafe { std::mem::zeroed() };
    unsafe { libc::malloc_zone_statistics(std::ptr::null_mut(), &raw mut stats) };
    stats.size_in_use
}

fn main() {
    let device = cpal::default_host().default_input_device().unwrap();
    for _ in 0..20 {
        drop(
            device
                .description()
                .map(|description| description.name().to_owned())
                .unwrap(),
        );
    }
    let baseline = live_bytes();
    for cycle in 1..=10000 {
        drop(
            device
                .description()
                .map(|description| description.name().to_owned())
                .unwrap(),
        );
        if cycle % 1000 == 0 {
            let retained = live_bytes().saturating_sub(baseline);
            println!("{cycle} name queries: {retained} additional live bytes");
        }
    }
    assert!(
        live_bytes().saturating_sub(baseline) < 128 * 1024,
        "device-name queries retain memory"
    );
}
