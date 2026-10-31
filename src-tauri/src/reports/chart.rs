//! Charts as inline SVG, so a report stays one self-contained file that prints well.

use std::fmt::Write;

const WIDTH: f64 = 640.0;
const ACCENT: &str = "#ea580c";
const MUTED: &str = "#a1a1aa";
const INK: &str = "#3f3f46";
pub const MAX_BARS: usize = 12;

pub struct Point {
    pub label: String,
    pub value: f64,
}

/// Horizontal bars with the value at the end of each one.
pub fn bar(points: &[Point], fmt: &dyn Fn(f64) -> String) -> String {
    let points = &points[..points.len().min(MAX_BARS)];
    let (label_w, value_w, row_h) = (170.0, 90.0, 30.0);
    let area = WIDTH - label_w - value_w;
    let max = points
        .iter()
        .map(|p| p.value.abs())
        .fold(0.0, f64::max)
        .max(f64::MIN_POSITIVE);
    let height = row_h * points.len() as f64;

    let mut svg = open(height);
    for (i, p) in points.iter().enumerate() {
        let y = i as f64 * row_h;
        let w = (p.value.abs() / max * area).max(1.0);
        let color = if p.value < 0.0 { MUTED } else { ACCENT };
        let _ = write!(
            svg,
            r#"<g><title>{title}: {value}</title><text x="{lx}" y="{ty}" text-anchor="end" fill="{INK}">{label}</text><rect x="{label_w}" y="{ry}" width="{w:.1}" height="18" rx="3" fill="{color}"/><text x="{vx:.1}" y="{ty}" fill="{INK}">{value}</text></g>"#,
            title = escape(&p.label),
            label = escape(&truncate(&p.label, 24)),
            value = escape(&fmt(p.value)),
            lx = label_w - 10.0,
            ty = y + 19.0,
            ry = y + 6.0,
            vx = label_w + w + 8.0,
        );
    }
    svg.push_str("</svg>");
    svg
}

/// A line over ordered periods, with a light area underneath.
pub fn line(points: &[Point], fmt: &dyn Fn(f64) -> String) -> String {
    let (left, right, top, bottom, height) = (72.0, 16.0, 12.0, 34.0, 240.0);
    let (w, h) = (WIDTH - left - right, height - top - bottom);
    let (lo, hi, step) = axis(points);
    let x = |i: usize| {
        left + if points.len() > 1 {
            i as f64 * w / (points.len() - 1) as f64
        } else {
            w / 2.0
        }
    };
    let y = |v: f64| top + h - (v - lo) / (hi - lo) * h;

    let mut svg = open(height);
    let ticks = ((hi - lo) / step).round() as u32;
    for tick in 0..=ticks {
        let v = lo + step * f64::from(tick);
        let _ = write!(
            svg,
            r##"<line x1="{left}" x2="{x2}" y1="{gy:.1}" y2="{gy:.1}" stroke="#e4e4e7"/><text x="{tx}" y="{ty:.1}" text-anchor="end" fill="{MUTED}">{label}</text>"##,
            x2 = left + w,
            gy = y(v),
            tx = left - 8.0,
            ty = y(v) + 4.0,
            label = escape(&fmt(v)),
        );
    }
    if !points.is_empty() {
        let line: Vec<String> = points
            .iter()
            .enumerate()
            .map(|(i, p)| format!("{:.1},{:.1}", x(i), y(p.value)))
            .collect();
        let _ = write!(
            svg,
            r#"<polygon points="{x0:.1},{base:.1} {line} {xn:.1},{base:.1}" fill="{ACCENT}" fill-opacity="0.08"/><polyline points="{line}" fill="none" stroke="{ACCENT}" stroke-width="2.5" stroke-linejoin="round"/>"#,
            x0 = x(0),
            xn = x(points.len() - 1),
            base = y(0.0_f64.clamp(lo, hi)),
            line = line.join(" "),
        );
    }
    let every = points.len().div_ceil(8).max(1);
    for (i, p) in points.iter().enumerate() {
        let _ = write!(
            svg,
            r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="3" fill="{ACCENT}"><title>{title}: {value}</title></circle>"#,
            cx = x(i),
            cy = y(p.value),
            title = escape(&p.label),
            value = escape(&fmt(p.value)),
        );
        let last = i == points.len() - 1;
        if i % every == 0 || last {
            // The edges are anchored inwards so their labels are not clipped.
            let anchor = if last && i > 0 {
                "end"
            } else if i == 0 {
                "start"
            } else {
                "middle"
            };
            let _ = write!(
                svg,
                r#"<text x="{tx:.1}" y="{ty}" text-anchor="{anchor}" fill="{MUTED}">{label}</text>"#,
                tx = x(i),
                ty = height - 10.0,
                label = escape(&truncate(&p.label, 10)),
            );
        }
    }
    svg.push_str("</svg>");
    svg
}

/// A value range that starts at zero (or below) and ends on a round number, with a round step.
fn axis(points: &[Point]) -> (f64, f64, f64) {
    let lo = points.iter().map(|p| p.value).fold(0.0, f64::min);
    let hi = points.iter().map(|p| p.value).fold(0.0, f64::max);
    let range = (hi - lo).max(f64::MIN_POSITIVE);
    let raw = range / 4.0;
    let magnitude = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .into_iter()
        .map(|n| n * magnitude)
        .find(|&s| s >= raw)
        .unwrap_or(10.0 * magnitude);
    (
        (lo / step).floor() * step,
        ((hi / step).ceil() * step).max(lo + step),
        step,
    )
}

fn open(height: f64) -> String {
    format!(
        r#"<svg class="chart" viewBox="0 0 {WIDTH} {height}" width="100%" role="img" xmlns="http://www.w3.org/2000/svg" font-size="12" font-family="inherit">"#
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(values: &[(&str, f64)]) -> Vec<Point> {
        values
            .iter()
            .map(|(l, v)| Point {
                label: (*l).into(),
                value: *v,
            })
            .collect()
    }

    #[test]
    fn bars_scale_to_the_largest_value_and_escape_labels() {
        let svg = bar(&points(&[("North", 100.0), ("<script>", 50.0)]), &|v| {
            v.to_string()
        });
        assert_eq!(svg.matches("<rect").count(), 2);
        assert!(svg.contains(r#"width="380.0""#), "{svg}");
        assert!(svg.contains(r#"width="190.0""#), "{svg}");
        assert!(svg.contains("&lt;script&gt;") && !svg.contains("<script>"));
    }

    #[test]
    fn bars_are_capped() {
        let many: Vec<_> = (0..30)
            .map(|i| Point {
                label: i.to_string(),
                value: f64::from(i),
            })
            .collect();
        assert_eq!(
            bar(&many, &|v| v.to_string()).matches("<rect").count(),
            MAX_BARS
        );
    }

    #[test]
    fn axes_use_round_steps() {
        assert_eq!(
            axis(&points(&[("a", 783_531.65), ("b", 300_768.0)])),
            (0.0, 800_000.0, 200_000.0)
        );
        assert_eq!(
            axis(&points(&[("a", -30.0), ("b", 70.0)])),
            (-50.0, 75.0, 25.0)
        );
    }

    #[test]
    fn line_has_one_point_per_period() {
        let svg = line(
            &points(&[("2026-01", 3.0), ("2026-02", 5.0), ("2026-03", 4.0)]),
            &|v| v.to_string(),
        );
        assert_eq!(svg.matches("<circle").count(), 3);
        assert!(svg.contains("<polyline"));
    }
}
