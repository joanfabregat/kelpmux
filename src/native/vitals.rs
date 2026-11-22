//! Status-bar vitals: KelpServer's structured `Vitals` (sent in every
//! `Update`) and its `Sampler`, plus KelpMux's presentation of them. The
//! presentation functions were copied from KelpServer's `native::vitals`
//! (kelpserver afc5197, Apache-2.0, same authors; the glyphs are adapted
//! from Vitals, MIT, see NOTICE), which removed them in 0.3.0-beta.2.
pub use kelpserver::native::vitals::*;

pub fn bar_glyph(percent: u8) -> char {
    match (u32::from(percent.min(100)) * 8 + 50) / 100 {
        0 | 1 => '▁',
        2 => '▂',
        3 => '▃',
        4 => '▄',
        5 => '▅',
        6 => '▆',
        7 => '▇',
        _ => '█',
    }
}
pub fn is_clock_segment(text: &str) -> bool {
    text.contains(':') || (text.len() == 10 && text.as_bytes().get(4) == Some(&b'-'))
}
pub fn pie_glyph(percent: u8) -> char {
    match percent {
        88.. => '●',
        63.. => '◕',
        38.. => '◑',
        13.. => '◔',
        _ => '○',
    }
}
/// Plain text and indexed foreground colors; callers own layout and styling.
pub fn render_segments(vitals: &Vitals) -> Vec<(String, u8)> {
    let mut segments = Vec::new();
    for (label, percent, color) in [
        ("cpu", vitals.cpu, 114),
        ("mem", vitals.memory, 141),
        ("swap", vitals.swap, 73),
    ] {
        if let Some(percent) = percent {
            let percent = percent.min(100);
            segments.push((format!("{label} {} {percent}%", bar_glyph(percent)), color));
        }
    }
    if !vitals.disks.is_empty() {
        segments.push(("disk".into(), 222));
    }
    for &percent in &vitals.disks {
        let percent = percent.min(100);
        segments.push((
            format!("{} {percent}%", pie_glyph(percent)),
            if percent >= 90 { 196 } else { 222 },
        ));
    }
    if let Some(date) = &vitals.date {
        segments.push((date.clone(), 39));
    }
    if let Some(time) = &vitals.time {
        segments.push((time.clone(), 250));
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Copied from KelpServer's `glyphs_color_segments_and_alert_boundary_match_vitals`.
    #[test]
    fn glyphs_color_segments_and_alert_boundary_match_vitals() {
        assert_eq!(
            [bar_glyph(0), bar_glyph(19), bar_glyph(50), bar_glyph(94)],
            ['▁', '▂', '▄', '█']
        );
        assert_eq!(
            [pie_glyph(12), pie_glyph(13), pie_glyph(63), pie_glyph(88)],
            ['○', '◔', '◕', '●']
        );
        let data = Vitals {
            cpu: Some(26),
            memory: Some(7),
            swap: Some(0),
            disks: vec![89, 90],
            date: Some("2026-10-05".into()),
            time: Some("21:01".into()),
        };
        assert_eq!(
            render_segments(&data),
            vec![
                ("cpu ▂ 26%".into(), 114),
                ("mem ▁ 7%".into(), 141),
                ("swap ▁ 0%".into(), 73),
                ("disk".into(), 222),
                ("● 89%".into(), 222),
                ("● 90%".into(), 196),
                ("2026-10-05".into(), 39),
                ("21:01".into(), 250)
            ]
        );
        assert!(render_segments(&Vitals::default()).is_empty());
        assert!(is_clock_segment("21:01") && is_clock_segment("2026-10-05"));
        assert!(!is_clock_segment("cpu ▂ 26%"));
    }
}
