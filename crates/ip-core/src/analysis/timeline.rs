//! Event layer (docs/03 §2.3, docs/09 P1-1): scenes of one session are cut into events, the
//! trips / outings / days the user thinks in. An event contains whole scenes, which in turn contain
//! bursts (`event -> scene -> burst -> photo`). Pure functions; SQL is in `store.rs`.
//!
//! A boundary between two neighbouring scenes (in capture order) is placed when
//! - the pause between them is longer than [`EVENT_GAP_MS`] (2 h), or
//! - the last known GPS position before the boundary and the first known one after it are more
//!   than [`EVENT_DISTANCE_M`] (5 km) apart.
//!
//! Photos without GPS never cut an event by distance (the last known position is carried over
//! them), so a session without any GPS is split by time only. Capture times are absolute (UTC
//! milliseconds), so a change of time zone between two photos does not look like a pause; the UTC
//! offset of the photos is only used to show the local wall-clock date and time.

use crate::export::civil_from_days;

/// Longest pause inside one event: a gap between two scenes above this starts a new event.
pub const EVENT_GAP_MS: i64 = 2 * 60 * 60 * 1000;
/// Largest distance between consecutive known GPS positions inside one event, in metres.
pub const EVENT_DISTANCE_M: f64 = 5_000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EventParams {
    pub gap_ms: i64,
    pub distance_m: f64,
}

impl Default for EventParams {
    fn default() -> Self {
        Self {
            gap_ms: EVENT_GAP_MS,
            distance_m: EVENT_DISTANCE_M,
        }
    }
}

/// `(latitude, longitude)` in degrees.
pub type LatLon = (f64, f64);

/// What event splitting needs to know about one scene.
#[derive(Debug, Clone, PartialEq)]
pub struct EventScene {
    /// Capture time (UTC ms) of the scene's first / last photo.
    pub start: i64,
    pub end: i64,
    /// GPS position of the earliest / latest photo of the scene that has one.
    pub first_gps: Option<LatLon>,
    pub last_gps: Option<LatLon>,
}

/// A usable GPS fix: finite, in range, and not the `0,0` that cameras write when they have none.
pub fn valid_gps(lat: Option<f64>, lon: Option<f64>) -> Option<LatLon> {
    let (lat, lon) = (lat?, lon?);
    if !lat.is_finite() || !lon.is_finite() || lat.abs() > 90.0 || lon.abs() > 180.0 {
        return None;
    }
    if lat == 0.0 && lon == 0.0 {
        return None;
    }
    Some((lat, lon))
}

/// Great-circle distance in metres (haversine, mean Earth radius).
pub fn distance_m(a: LatLon, b: LatLon) -> f64 {
    const R: f64 = 6_371_008.8;
    let (p1, p2) = (a.0.to_radians(), b.0.to_radians());
    let dp = p2 - p1;
    let dl = (b.1 - a.1).to_radians();
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * R * h.sqrt().min(1.0).asin()
}

/// Splits capture-ordered `scenes` into events; returns index lists (each non-empty, in order).
pub fn split_events(scenes: &[EventScene], p: &EventParams) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut latest_end = i64::MIN;
    let mut last_pos: Option<LatLon> = None;
    for (i, s) in scenes.iter().enumerate() {
        let cut = i > 0
            && (s.start.saturating_sub(latest_end) > p.gap_ms
                || matches!((last_pos, s.first_gps), (Some(a), Some(b)) if distance_m(a, b) > p.distance_m));
        if cut || out.is_empty() {
            out.push(vec![i]);
        } else {
            out.last_mut().unwrap().push(i);
        }
        latest_end = latest_end.max(s.end);
        if s.last_gps.is_some() {
            last_pos = s.last_gps;
        }
    }
    out
}

/// Default display name of an event: local date and time span, `2024-05-03 10:12-14:40`; an event
/// that crosses midnight is `2024-05-03 22:10-05-04 01:20`. `offset_min` is the UTC offset the
/// photos were taken at; `None` = unknown (the times are then taken as they are).
pub fn event_name(start: i64, end: i64, offset_min: Option<i32>) -> String {
    let shift = offset_min.unwrap_or(0) as i64 * 60_000;
    let (s, e) = (start + shift, end.max(start) + shift);
    let (ds, de) = (s.div_euclid(86_400_000), e.div_euclid(86_400_000));
    let (y, mo, d) = civil_from_days(ds);
    let hm = |ms: i64| {
        let m = ms.rem_euclid(86_400_000) / 60_000;
        format!("{:02}:{:02}", m / 60, m % 60)
    };
    if ds == de {
        if hm(s) == hm(e) {
            format!("{y:04}-{mo:02}-{d:02} {}", hm(s))
        } else {
            format!("{y:04}-{mo:02}-{d:02} {}-{}", hm(s), hm(e))
        }
    } else {
        let (ye, moe, de_) = civil_from_days(de);
        if ye == y {
            format!(
                "{y:04}-{mo:02}-{d:02} {}-{moe:02}-{de_:02} {}",
                hm(s),
                hm(e)
            )
        } else {
            format!(
                "{y:04}-{mo:02}-{d:02} {}-{ye:04}-{moe:02}-{de_:02} {}",
                hm(s),
                hm(e)
            )
        }
    }
}

/// [`event_name`] for file names: no `:`.
pub fn event_file_name(name: &str) -> String {
    name.replace(':', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;
    const H: i64 = 60 * MIN;
    const TOKYO: LatLon = (35.6762, 139.6503);
    // about 3.3 km east of TOKYO (inside 5 km) and about 11 km east (outside)
    const NEAR: LatLon = (35.6762, 139.6860);
    const FAR: LatLon = (35.6762, 139.7700);

    fn scene(start: i64, end: i64, gps: Option<LatLon>) -> EventScene {
        EventScene {
            start,
            end,
            first_gps: gps,
            last_gps: gps,
        }
    }

    #[test]
    fn distance_is_great_circle() {
        assert!(distance_m(TOKYO, TOKYO) < 1e-6);
        let d = distance_m(TOKYO, NEAR);
        assert!((3_000.0..3_600.0).contains(&d), "{d}");
        let d = distance_m(TOKYO, FAR);
        assert!((10_000.0..12_000.0).contains(&d), "{d}");
        // Tokyo - Osaka is about 400 km; the antimeridian is handled
        assert!((390_000.0..410_000.0).contains(&distance_m(TOKYO, (34.6937, 135.5023))));
        assert!(distance_m((0.1, 179.99), (0.1, -179.99)) < 5_000.0);
    }

    #[test]
    fn invalid_gps_is_ignored() {
        assert_eq!(valid_gps(Some(1.0), Some(2.0)), Some((1.0, 2.0)));
        assert_eq!(valid_gps(Some(0.0), Some(0.0)), None);
        assert_eq!(valid_gps(Some(91.0), Some(0.0)), None);
        assert_eq!(valid_gps(Some(f64::NAN), Some(0.0)), None);
        assert_eq!(valid_gps(None, Some(3.0)), None);
    }

    #[test]
    fn time_gap_splits_events() {
        let p = EventParams::default();
        let s = vec![
            scene(0, 10 * MIN, None),
            scene(10 * MIN + 119 * MIN, 140 * MIN, None), // 119 min later: same event
            scene(140 * MIN + 2 * H + 1, 300 * MIN, None), // 2 h + 1 ms later: new event
            scene(300 * MIN + 2 * H, 330 * MIN, None),    // exactly 2 h: same event
        ];
        assert_eq!(split_events(&s, &p), vec![vec![0, 1], vec![2, 3]]);
        assert!(split_events(&[], &p).is_empty());
        assert_eq!(split_events(&s[..1], &p), vec![vec![0]]);
    }

    #[test]
    fn gps_distance_splits_events_without_a_pause() {
        let p = EventParams::default();
        let s = vec![
            scene(0, MIN, Some(TOKYO)),
            scene(10 * MIN, 11 * MIN, Some(NEAR)), // 3.3 km: same event
            scene(20 * MIN, 21 * MIN, Some(FAR)),  // 7.7 km from NEAR: new event
            scene(30 * MIN, 31 * MIN, Some(FAR)),
        ];
        assert_eq!(split_events(&s, &p), vec![vec![0, 1], vec![2, 3]]);
    }

    #[test]
    fn photos_without_gps_never_cut_by_distance() {
        let p = EventParams::default();
        // the middle scene has no GPS: the position before and after it is compared
        let s = vec![
            scene(0, MIN, Some(TOKYO)),
            scene(10 * MIN, 11 * MIN, None),
            scene(20 * MIN, 21 * MIN, Some(FAR)),
        ];
        assert_eq!(split_events(&s, &p), vec![vec![0, 1], vec![2]]);
        // no GPS at all: time only
        let s = vec![scene(0, MIN, None), scene(10 * MIN, 11 * MIN, None)];
        assert_eq!(split_events(&s, &p), vec![vec![0, 1]]);
        // GPS only on one side: nothing to compare
        let s = vec![scene(0, MIN, Some(TOKYO)), scene(10 * MIN, 11 * MIN, None)];
        assert_eq!(split_events(&s, &p), vec![vec![0, 1]]);
    }

    #[test]
    fn a_scene_that_moves_compares_its_last_position() {
        let p = EventParams::default();
        // a drive: the scene starts in Tokyo and ends far away; the next scene is near where it ended
        let drive = EventScene {
            start: 0,
            end: 9 * MIN,
            first_gps: Some(TOKYO),
            last_gps: Some(FAR),
        };
        let s = vec![drive, scene(20 * MIN, 21 * MIN, Some(FAR))];
        assert_eq!(split_events(&s, &p), vec![vec![0, 1]]);
    }

    #[test]
    fn overlapping_scenes_use_the_latest_end() {
        let p = EventParams::default();
        let s = vec![
            scene(0, 6 * H, None),
            scene(MIN, 2 * MIN, None),
            scene(7 * H, 7 * H + MIN, None), // 1 h after the long scene ended
            scene(10 * H, 10 * H + MIN, None), // 3 h later
        ];
        assert_eq!(split_events(&s, &p), vec![vec![0, 1, 2], vec![3]]);
    }

    #[test]
    fn custom_thresholds_apply() {
        let p = EventParams {
            gap_ms: 30 * MIN,
            distance_m: 100_000.0,
        };
        let s = vec![
            scene(0, MIN, Some(TOKYO)),
            scene(40 * MIN, 41 * MIN, Some(FAR)),
        ];
        assert_eq!(split_events(&s, &p).len(), 2);
    }

    /// 2024-05-03T00:00:00Z
    const D: i64 = 1_714_694_400_000;

    #[test]
    fn names_use_the_local_wall_clock() {
        // 02:30-06:10 UTC at UTC+9 is 11:30-15:10 the same day
        assert_eq!(
            event_name(D + 2 * H + 30 * MIN, D + 6 * H + 10 * MIN, Some(540)),
            "2024-05-03 11:30-15:10"
        );
        // unknown offset: the stored value is already the wall clock
        assert_eq!(
            event_name(D + 2 * H, D + 3 * H, None),
            "2024-05-03 02:00-03:00"
        );
        // UTC date and local date differ: 20:00 UTC on the 3rd is 05:00 on the 4th in Tokyo
        assert_eq!(
            event_name(D + 20 * H, D + 20 * H + 5 * MIN, Some(540)),
            "2024-05-04 05:00-05:05"
        );
        // negative offsets (New York in summer, UTC-4)
        assert_eq!(
            event_name(D + 2 * H, D + 3 * H, Some(-240)),
            "2024-05-02 22:00-23:00"
        );
        // a single instant
        assert_eq!(event_name(D + H, D + H, Some(0)), "2024-05-03 01:00");
    }

    #[test]
    fn names_of_events_across_midnight_or_new_year() {
        assert_eq!(
            event_name(D + 22 * H, D + 25 * H, Some(0)),
            "2024-05-03 22:00-05-04 01:00"
        );
        let ny = 1_735_689_600_000; // 2025-01-01T00:00:00Z
        assert_eq!(
            event_name(ny - H, ny + H, Some(0)),
            "2024-12-31 23:00-2025-01-01 01:00"
        );
        assert_eq!(
            event_file_name("2024-05-03 11:30-15:10"),
            "2024-05-03 1130-1510"
        );
    }
}
