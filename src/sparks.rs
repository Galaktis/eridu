//! Deleted files burst into glowing sparks of their own color, which fly out, fall a little
//! and fade. Sparks are drawn as extra instances of the file layer: no entity per spark.

use bevy::prelude::*;

use crate::files::FileInstance;

/// Sparks per deleted file once many are alive (mass deletions stay cheap); the usual count
/// is `Sparks::per_file`, set by the quality tier.
const SPARKS_UNDER_LOAD: usize = 2;
/// Hard cap on live sparks.
const MAX_SPARKS: usize = 20_000;
const SPARK_SECONDS: f64 = 1.2;
const SPARK_SPEED: f32 = 5.0;
/// Velocity lost per second (exponential drag), and the pull downwards.
const SPARK_DRAG: f32 = 2.5;
const SPARK_GRAVITY: f32 = 3.0;
/// Size relative to a file, and emissive strength, at birth.
const SPARK_SCALE: f32 = 0.6;
const SPARK_EMISSIVE: f32 = 10.0;

struct Spark {
    pos: Vec3,
    vel: Vec3,
    color: LinearRgba,
    born: f64,
}

#[derive(Resource)]
pub struct Sparks {
    /// Sparks per deleted file (0 turns them off).
    pub per_file: usize,
    list: Vec<Spark>,
    /// xorshift state: sparks don't need a real random generator.
    seed: u32,
}

impl Default for Sparks {
    fn default() -> Self {
        Self {
            per_file: 8,
            list: Vec::new(),
            seed: 0x9e37_79b9,
        }
    }
}

impl Sparks {
    pub fn burst(&mut self, pos: Vec3, color: LinearRgba, now: f64) {
        let count = match self.list.len() {
            n if n >= MAX_SPARKS => return,
            n if n >= MAX_SPARKS / 2 => self.per_file.min(SPARKS_UNDER_LOAD),
            _ => self.per_file,
        };
        for _ in 0..count {
            let dir = Vec3::new(self.random(), self.random(), self.random()).normalize_or(Vec3::Y);
            let speed = SPARK_SPEED * (0.5 + 0.5 * self.random().abs());
            self.list.push(Spark {
                pos,
                vel: dir * speed,
                color,
                born: now,
            });
        }
    }

    /// A pseudo-random number in [-1, 1].
    fn random(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }

    pub fn update(&mut self, now: f64, dt: f32) {
        self.list.retain(|s| now - s.born < SPARK_SECONDS);
        let drag = (-SPARK_DRAG * dt).exp();
        for spark in &mut self.list {
            spark.vel = spark.vel * drag - Vec3::Y * SPARK_GRAVITY * dt;
            spark.pos += spark.vel * dt;
        }
    }

    pub fn instances(&self, now: f64, out: &mut Vec<FileInstance>) {
        out.extend(self.list.iter().map(|s| {
            let life = 1.0 - ((now - s.born) / SPARK_SECONDS) as f32;
            FileInstance {
                position: s.pos,
                scale: SPARK_SCALE * life,
                color: [
                    s.color.red,
                    s.color.green,
                    s.color.blue,
                    SPARK_EMISSIVE * life * life,
                ],
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst(sparks: &mut Sparks, now: f64) {
        sparks.burst(Vec3::ONE, LinearRgba::RED, now);
    }

    #[test]
    fn a_deletion_bursts_into_sparks() {
        let mut sparks = Sparks::default();
        burst(&mut sparks, 0.0);
        assert_eq!(sparks.list.len(), sparks.per_file);
        for spark in &sparks.list {
            assert_eq!(spark.pos, Vec3::ONE);
            let speed = spark.vel.length();
            assert!((0.5 * SPARK_SPEED - 1e-4..=SPARK_SPEED + 1e-4).contains(&speed));
        }
        // Flying in different directions.
        let first = sparks.list[0].vel.normalize();
        assert!(
            sparks
                .list
                .iter()
                .any(|s| s.vel.normalize().dot(first) < 0.9)
        );
    }

    #[test]
    fn no_sparks_when_turned_off() {
        let mut sparks = Sparks {
            per_file: 0,
            ..default()
        };
        burst(&mut sparks, 0.0);
        assert!(sparks.list.is_empty());
    }

    #[test]
    fn mass_deletions_stay_capped() {
        let mut sparks = Sparks::default();
        while sparks.list.len() < MAX_SPARKS / 2 {
            burst(&mut sparks, 0.0);
        }
        let before = sparks.list.len();
        burst(&mut sparks, 0.0);
        assert_eq!(sparks.list.len() - before, SPARKS_UNDER_LOAD);
        for _ in 0..MAX_SPARKS {
            burst(&mut sparks, 0.0);
        }
        assert!(sparks.list.len() < MAX_SPARKS + SPARKS_UNDER_LOAD);
    }

    #[test]
    fn sparks_slow_down_fall_and_die() {
        let mut sparks = Sparks::default();
        burst(&mut sparks, 0.0);
        let start: Vec<Vec3> = sparks.list.iter().map(|s| s.vel).collect();
        let dt = 1.0 / 60.0;
        let mut now = 0.0;
        for _ in 0..30 {
            now += f64::from(dt);
            sparks.update(now, dt);
        }
        // Half a second: drag alone would leave this much of the speed…
        let dragged = (-SPARK_DRAG * 0.5).exp();
        for (spark, v) in sparks.list.iter().zip(&start) {
            assert!((spark.vel.x - v.x * dragged).abs() < 1e-3);
            assert!((spark.vel.z - v.z * dragged).abs() < 1e-3);
            // …and gravity pulls downwards on top of it.
            assert!(spark.vel.y < v.y * dragged - 0.5);
        }
        sparks.update(SPARK_SECONDS + 0.01, dt);
        assert!(sparks.list.is_empty());
    }

    #[test]
    fn sparks_shrink_and_fade() {
        let mut sparks = Sparks::default();
        burst(&mut sparks, 0.0);
        let mut out = Vec::new();
        sparks.instances(0.0, &mut out);
        assert_eq!(out.len(), sparks.per_file);
        assert_eq!(
            (out[0].scale, out[0].color[3]),
            (SPARK_SCALE, SPARK_EMISSIVE)
        );
        out.clear();
        sparks.instances(SPARK_SECONDS / 2.0, &mut out);
        assert!((out[0].scale - SPARK_SCALE / 2.0).abs() < 1e-5);
        assert!((out[0].color[3] - SPARK_EMISSIVE / 4.0).abs() < 1e-4);
        assert_eq!(out[0].color[..3], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn random_spreads_over_minus_one_to_one() {
        let mut sparks = Sparks::default();
        let values: Vec<f32> = (0..10_000).map(|_| sparks.random()).collect();
        assert!(values.iter().all(|v| (-1.0..=1.0).contains(v)));
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        assert!(mean.abs() < 0.05, "{mean}");
        assert!(values.iter().any(|&v| v < -0.9) && values.iter().any(|&v| v > 0.9));
    }

    #[test]
    fn clear_drops_every_spark() {
        let mut sparks = Sparks::default();
        burst(&mut sparks, 0.0);
        sparks.clear();
        assert!(sparks.list.is_empty());
    }
}
