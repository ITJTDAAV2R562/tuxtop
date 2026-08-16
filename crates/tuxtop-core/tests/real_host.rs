//! Parser checks against a real `/proc/stat`, not a hand-written fixture.
//!
//! `fixtures/dove-stat.txt` holds two consecutive readings from dove (a
//! 32-core Debian 13 box) captured one second apart, plus the `top` output
//! from the same moment as an independent ground truth.
//!
//! Hand-written fixtures verify the arithmetic; this file verifies that the
//! arithmetic is being applied to a correctly parsed real kernel file. Both
//! matter — the Beszel investigation that started this project went wrong in
//! exactly the gap between "the maths is right" and "the input is what I think
//! it is".

use tuxtop_core::{busy_pct, core_pcts, parse_stat};

const FIXTURE: &str = include_str!("fixtures/dove-stat.txt");

/// Split the fixture into (first reading, second reading, top's idle figure).
fn parts() -> (&'static str, &'static str, f32) {
    let (first, rest) = FIXTURE
        .split_once("===SECOND===")
        .expect("fixture has two readings");
    let (second, top) = rest
        .split_once("===TOP===")
        .expect("fixture has top output");

    // "%Cpu(s):  0.0 us,  0.3 sy,  0.0 ni, 99.7 id, ..." -> 99.7
    let idle = top
        .split(',')
        .find(|f| f.trim_end().ends_with("id"))
        .and_then(|f| f.trim().split_ascii_whitespace().next())
        .and_then(|v| v.parse::<f32>().ok())
        .expect("top reported an idle percentage");

    (first, second, idle)
}

#[test]
fn parses_all_32_cores_of_a_real_host() {
    let (first, _, _) = parts();
    let snap = parse_stat(first);
    assert_eq!(snap.cores.len(), 32, "dove has 32 logical cores");
    assert!(snap.aggregate.total_jiffies() > 0);
}

#[test]
fn aggregate_equals_the_sum_of_the_cores() {
    // The kernel computes the `cpu` row independently of the `cpuN` rows, so
    // this cross-checks that we are reading both correctly. They can differ by
    // a jiffy or two from rounding as counters advance mid-read.
    let (first, _, _) = parts();
    let snap = parse_stat(first);

    let summed: u64 = snap.cores.iter().map(|c| c.total_jiffies()).sum();
    let agg = snap.aggregate.total_jiffies();
    let drift = agg.abs_diff(summed);

    assert!(
        drift < agg / 1000,
        "aggregate {agg} vs summed cores {summed} drifted by {drift}"
    );
}

#[test]
fn computed_busy_matches_what_top_reported() {
    let (first, second, top_idle) = parts();
    let a = parse_stat(first);
    let b = parse_stat(second);

    let ours = busy_pct(&a.aggregate, &b.aggregate);
    let theirs = 100.0 - top_idle;

    // Both sampled the same second from the same kernel counters, so they
    // should land close. The tolerance covers the small window between our two
    // cats and top's own sampling.
    assert!(
        (ours - theirs).abs() < 5.0,
        "we computed {ours:.2}% busy, top said {theirs:.2}%"
    );
}

#[test]
fn every_core_lands_in_range() {
    let (first, second, _) = parts();
    let pcts = core_pcts(&parse_stat(first), &parse_stat(second));

    assert_eq!(pcts.len(), 32);
    for (i, p) in pcts.iter().enumerate() {
        assert!(p.is_finite(), "cpu{i} produced {p}");
        assert!((0.0..=100.0).contains(p), "cpu{i} = {p}, outside 0..=100");
    }
}
