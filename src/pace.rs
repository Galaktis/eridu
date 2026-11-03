//! How busy the history is on screen, and how the view adapts to it. A calm stretch (a slow
//! speed, a small team) lets glows, beams and authors linger; a busy one (a big repository
//! played fast) fades them sooner and draws a beam for only a share of the changes, so the
//! screen never turns into a web of lines. Everything eases, so the look changes smoothly.

use bevy::prelude::*;

/// About this many beams on screen at once, at most: `changes per second × glow seconds`.
const BEAM_BUDGET: f64 = 60.0;
/// A change glows (and its beam lasts) this long: long when calm, short when busy.
const MAX_GLOW_SECONDS: f64 = 1.5;
const MIN_GLOW_SECONDS: f64 = 0.4;
/// Even at the busiest, this share of changes keeps a beam.
const MIN_BEAM_SHARE: f64 = 0.05;
/// About this many commits' authors on the halo: `commits per second × idle seconds` (the
/// halo itself holds `MAX_SHOWN_AUTHORS`).
const AUTHOR_BUDGET: f64 = 30.0;
/// An author stays on the halo this long after their last change.
const MAX_IDLE_SECONDS: f64 = 10.0;
const MIN_IDLE_SECONDS: f64 = 4.0;
/// Rates are averaged over about this long, and the settings ease over about this long.
const RATE_SECONDS: f64 = 2.0;
const EASE_SECONDS: f64 = 1.5;

#[derive(Resource)]
pub struct Pace {
    /// File changes and commits per second of animation, smoothed.
    changes: f64,
    commits: f64,
    /// How long a change glows and its beam lasts, in seconds.
    pub glow: f64,
    /// The share of changes drawn with a beam.
    pub beams: f64,
    /// How long an author stays on the halo after their last change, in seconds.
    pub idle: f64,
}

impl Default for Pace {
    fn default() -> Self {
        Self {
            changes: 0.0,
            commits: 0.0,
            glow: MAX_GLOW_SECONDS,
            beams: 1.0,
            idle: MAX_IDLE_SECONDS,
        }
    }
}

impl Pace {
    /// Takes in one frame of playback: `changes` applied and `commits` reached in `dt` seconds.
    pub fn update(&mut self, changes: usize, commits: usize, dt: f64) {
        if dt <= 0.0 {
            return;
        }
        let k = (dt / RATE_SECONDS).min(1.0);
        self.changes += (changes as f64 / dt - self.changes) * k;
        self.commits += (commits as f64 / dt - self.commits) * k;
        let glow = (BEAM_BUDGET / self.changes.max(f64::EPSILON))
            .clamp(MIN_GLOW_SECONDS, MAX_GLOW_SECONDS);
        let beams =
            (BEAM_BUDGET / (self.changes * glow).max(f64::EPSILON)).clamp(MIN_BEAM_SHARE, 1.0);
        let idle = (AUTHOR_BUDGET / self.commits.max(f64::EPSILON))
            .clamp(MIN_IDLE_SECONDS, MAX_IDLE_SECONDS);
        let ease = (dt / EASE_SECONDS).min(1.0);
        self.glow += (glow - self.glow) * ease;
        self.beams += (beams - self.beams) * ease;
        self.idle += (idle - self.idle) * ease;
    }

    /// Whether the change that `a` and `b` identify (a file and its commit, say) gets a beam:
    /// a stable pick, so a beam doesn't flicker while it fades.
    pub fn beam(&self, a: u64, b: u64) -> bool {
        self.beams >= 1.0 || unit(a, b) < self.beams
    }

    /// For the `F3` overlay.
    pub fn describe(&self) -> String {
        format!(
            "{:.0} changes/s, {:.1} commits/s: glow {:.2} s, beams {:.0}%, authors stay {:.1} s",
            self.changes,
            self.commits,
            self.glow,
            self.beams * 100.0,
            self.idle
        )
    }
}

/// A well-mixed number in [0, 1) from two (SplitMix64's finalizer).
fn unit(a: u64, b: u64) -> f64 {
    let mut h = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_add(0xD1B5_4A32_D192_ED03);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// 1 when something was just touched, fading to 0 over `seconds`.
pub fn glow(now: f64, touched_at: f64, seconds: f64) -> f32 {
    (1.0 - (now - touched_at) / seconds).max(0.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_fades_sooner_and_thins_beams() {
        let mut calm = Pace::default();
        let mut busy = Pace::default();
        for _ in 0..600 {
            calm.update(1, 0, 0.5);
            busy.update(50, 2, 1.0 / 60.0);
        }
        // Two changes a second: everything lingers.
        assert!(calm.glow > 1.4 && calm.beams > 0.99 && calm.idle > 9.0);
        // 3,000 changes and 120 commits a second: short glows, a few beams, short stays.
        assert!(busy.glow < 0.45 && busy.beams < 0.1 && busy.idle < 4.5);
        // About the share asked for gets a beam.
        let kept = (0..10_000).filter(|&i| busy.beam(i, 7)).count() as f64 / 10_000.0;
        assert!((kept - busy.beams).abs() < 0.02);
    }

    /// A pace after a long steady stretch of `changes` and `commits` per second.
    fn steady(changes: usize, commits: usize) -> Pace {
        let mut pace = Pace::default();
        for _ in 0..2000 {
            pace.update(changes, commits, 1.0);
        }
        pace
    }

    #[test]
    fn starts_calm() {
        let pace = Pace::default();
        assert_eq!(
            (pace.glow, pace.beams, pace.idle),
            (MAX_GLOW_SECONDS, 1.0, MAX_IDLE_SECONDS)
        );
        assert!((0..100).all(|i| pace.beam(i, i)));
    }

    #[test]
    fn rates_follow_the_frames() {
        let pace = steady(80, 3);
        assert!((pace.changes - 80.0).abs() < 1e-6 && (pace.commits - 3.0).abs() < 1e-6);
    }

    #[test]
    fn settings_meet_their_budgets_between_the_bounds() {
        // 80 changes/s: 60 beams on screen with every change glowing 0.75 s.
        let pace = steady(80, 3);
        assert!((pace.glow - 0.75).abs() < 1e-6);
        assert!((pace.beams - 1.0).abs() < 1e-6);
        // 3 commits/s, 30 authors: 10 s, the most.
        assert!((pace.idle - MAX_IDLE_SECONDS).abs() < 1e-6);
        // 300 changes/s: the shortest glow, and then a beam for 60 / (300 × 0.4) of them.
        let pace = steady(300, 5);
        assert!((pace.glow - MIN_GLOW_SECONDS).abs() < 1e-6);
        assert!((pace.beams - 0.5).abs() < 1e-6);
        assert!((pace.idle - 6.0).abs() < 1e-6);
        // Far busier still: the floors.
        let pace = steady(1_000_000, 1000);
        assert!((pace.beams - MIN_BEAM_SHARE).abs() < 1e-6);
        assert!((pace.idle - MIN_IDLE_SECONDS).abs() < 1e-6);
    }

    #[test]
    fn settings_stay_in_bounds_whatever_happens() {
        let mut pace = Pace::default();
        let mut seed = 1u64;
        for i in 0..5000 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let changes = (seed >> 40) as usize % 5000;
            let commits = (seed >> 20) as usize % 50;
            let dt = [1.0 / 144.0, 1.0 / 60.0, 0.1, 3.0][i % 4];
            pace.update(changes, commits, dt);
            // Up to rounding.
            let within = |v: f64, min: f64, max: f64| (min - 1e-9..=max + 1e-9).contains(&v);
            assert!(within(pace.glow, MIN_GLOW_SECONDS, MAX_GLOW_SECONDS));
            assert!(within(pace.beams, MIN_BEAM_SHARE, 1.0));
            assert!(within(pace.idle, MIN_IDLE_SECONDS, MAX_IDLE_SECONDS));
        }
    }

    #[test]
    fn a_busy_frame_eases_in() {
        let mut pace = Pace::default();
        pace.update(10_000, 100, 1.0 / 60.0);
        // One frame moves the settings only a little of the way.
        assert!(pace.glow > 1.4 && pace.beams > 0.95 && pace.idle > 9.8);
    }

    #[test]
    fn frames_without_time_are_ignored() {
        let mut pace = Pace::default();
        pace.update(1000, 10, 0.0);
        pace.update(1000, 10, -1.0);
        assert_eq!((pace.changes, pace.commits), (0.0, 0.0));
        assert_eq!(pace.glow, MAX_GLOW_SECONDS);
    }

    #[test]
    fn beam_picks_are_stable() {
        let pace = steady(300, 5);
        let picks: Vec<bool> = (0..1000).map(|i| pace.beam(i, 3)).collect();
        assert_eq!(
            picks,
            (0..1000).map(|i| pace.beam(i, 3)).collect::<Vec<_>>()
        );
        // A different commit picks other files.
        assert_ne!(
            picks,
            (0..1000).map(|i| pace.beam(i, 4)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn unit_spreads_over_zero_to_one() {
        let mut buckets = [0usize; 10];
        for a in 0..100 {
            for b in 0..100 {
                let u = unit(a, b);
                assert!((0.0..1.0).contains(&u));
                buckets[(u * 10.0) as usize] += 1;
            }
        }
        assert!(
            buckets.iter().all(|&n| (850..1150).contains(&n)),
            "{buckets:?}"
        );
    }

    #[test]
    fn glow_fades_linearly_to_zero() {
        assert_eq!(glow(5.0, 5.0, 2.0), 1.0);
        assert_eq!(glow(6.0, 5.0, 2.0), 0.5);
        assert_eq!(glow(7.0, 5.0, 2.0), 0.0);
        assert_eq!(glow(100.0, 5.0, 2.0), 0.0);
        // Never touched.
        assert_eq!(glow(5.0, f64::NEG_INFINITY, 2.0), 0.0);
    }

    #[test]
    fn describe_shows_the_values() {
        let text = steady(300, 5).describe();
        assert_eq!(
            text,
            "300 changes/s, 5.0 commits/s: glow 0.40 s, beams 50%, authors stay 6.0 s"
        );
    }
}
