//! Timeline time: signed microseconds. Integer time keeps edits exact; seconds
//! only appear at the edges (ffmpeg arguments, the clock).

pub type Time = i64;

pub const SECOND: Time = 1_000_000;

pub fn secs(t: Time) -> f64 {
    t as f64 / SECOND as f64
}

pub fn from_secs(s: f64) -> Time {
    (s * SECOND as f64).round() as Time
}

/// The length of one frame at `fps`.
pub fn frame(fps: u32) -> Time {
    SECOND / fps.max(1) as Time
}

/// Snaps `t` down onto the frame grid.
pub fn floor_to_frame(t: Time, fps: u32) -> Time {
    let fps = fps.max(1) as i128;
    let index = (t as i128 * fps).div_euclid(SECOND as i128);
    (index * SECOND as i128 / fps) as Time
}

/// `mm:ss:ff`, or `h:mm:ss:ff` past an hour.
pub fn timecode(t: Time, fps: u32) -> String {
    let t = t.max(0);
    let fps = fps.max(1) as i64;
    let total_frames = (t as i128 * fps as i128 / SECOND as i128) as i64;
    let ff = total_frames % fps;
    let total_secs = total_frames / fps;
    let (h, m, s) = (total_secs / 3600, (total_secs / 60) % 60, total_secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}:{ff:02}")
    } else {
        format!("{m:02}:{s:02}:{ff:02}")
    }
}

/// A short label for ruler ticks: `0s`, `5s`, `1:05`.
pub fn ruler_label(t: Time) -> String {
    let total = t.max(0) / SECOND;
    if total < 60 {
        format!("{total}s")
    } else {
        format!("{}:{:02}", total / 60, total % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecodes() {
        assert_eq!(timecode(0, 30), "00:00:00");
        assert_eq!(timecode(SECOND + SECOND / 2, 30), "00:01:15");
        assert_eq!(timecode(61 * SECOND, 25), "01:01:00");
        assert_eq!(timecode(3600 * SECOND + SECOND, 30), "1:00:01:00");
    }

    #[test]
    fn frame_grid() {
        assert_eq!(floor_to_frame(SECOND / 30 + 5, 30), SECOND / 30);
        assert_eq!(floor_to_frame(0, 24), 0);
    }
}
