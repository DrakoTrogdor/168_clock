// 168-Hour "Week Clock" rendered with wgpu.
//
// The face is one full week = 7 days x 24 hours = 168 hours.
//   * 12 o'clock (top) is Sunday 00:00:00.
//   * The HOUR hand makes ONE full revolution per WEEK. It points at the
//     current hour-of-week (0..168), so each of the 168 hour ticks is one hour.
//   * The MINUTE hand is normal: one revolution per hour (60 divisions).
//   * The SECOND hand is normal: one revolution per minute (60 divisions).
//   * There are tick marks only for the 168 hours, none for minutes/seconds.
//
// Everything is drawn as colored triangles in a simple 2D pipeline. Geometry is
// rebuilt on the CPU every frame (it's tiny) and uploaded to one vertex buffer.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use chrono::{Datelike, Local, Timelike};
use winit::{
    dpi::{LogicalSize, PhysicalSize},
    event::{ElementState, Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowBuilder},
};

// Win32 calls used to clip the window to the clock's circle in fullscreen, so
// everything outside the circle shows the desktop. This shaping approach avoids
// GPU per-pixel alpha, which is unreliable with Windows DXGI flip-model
// swapchains, and the non-rectangular region forces DWM composition so it works
// in fullscreen too.
#[cfg(windows)]
#[link(name = "gdi32")]
extern "system" {
    fn CreateEllipticRgn(x1: i32, y1: i32, x2: i32, y2: i32) -> isize; // -> HRGN
}
#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn SetWindowRgn(hwnd: isize, hrgn: isize, redraw: i32) -> i32;
    fn SendMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
}
#[cfg(windows)]
#[link(name = "shell32")]
extern "system" {
    // Pull the large + small icons from the .exe's own embedded icon resource.
    fn ExtractIconExW(file: *const u16, index: i32, large: *mut isize, small: *mut isize, n: u32) -> u32;
}

/// Set the window's large (taskbar) and small (title-bar) icons from the icon
/// embedded in the .exe. winit's `with_window_icon` only sets the small icon, so
/// without this the taskbar uses ICON_BIG (unset) and falls back to a generic
/// icon. No-op if the icon can't be extracted.
#[cfg(windows)]
fn set_windows_taskbar_icon(hwnd: isize) {
    use std::os::windows::ffi::OsStrExt;
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let wide: Vec<u16> = exe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut large: isize = 0;
    let mut small: isize = 0;
    const WM_SETICON: u32 = 0x0080;
    const ICON_SMALL: usize = 0;
    const ICON_BIG: usize = 1;
    unsafe {
        if ExtractIconExW(wide.as_ptr(), 0, &mut large, &mut small, 1) == 0 {
            return;
        }
        if large != 0 {
            SendMessageW(hwnd, WM_SETICON, ICON_BIG, large);
        }
        if small != 0 {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL, small);
        }
    }
}

const SAMPLE_COUNT: u32 = 4; // 4x MSAA for smooth curves and thin hands.

// Day names, clockwise from the top (Sunday).
const DAY_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

// One distinct color per day, going clockwise from the top (Sunday).
const DAY_COLORS: [[f32; 3]; 7] = [
    [0.90, 0.22, 0.27], // Sun  - red
    [0.95, 0.45, 0.17], // Mon  - orange
    [0.97, 0.78, 0.31], // Tue  - yellow
    [0.42, 0.73, 0.40], // Wed  - green
    [0.20, 0.66, 0.60], // Thu  - teal
    [0.27, 0.50, 0.78], // Fri  - blue
    [0.62, 0.35, 0.85], // Sat  - purple
];

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    color: [f32; 3],
}

const SHADER: &str = r#"
struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec3<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(in.pos, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
"#;

// ---------------------------------------------------------------------------
// Geometry helpers. All geometry is built in "clock space": a square centered
// on the origin spanning roughly [-1, 1]. Angles are measured CLOCKWISE from
// the top (12 o'clock), matching a real clock.
// ---------------------------------------------------------------------------

/// Unit direction for a fraction `f` of a full turn, clockwise from the top.
/// f = 0.0 -> straight up, f = 0.25 -> right, f = 0.5 -> down.
fn dir(f: f64) -> (f32, f32) {
    let t = f * std::f64::consts::TAU;
    (t.sin() as f32, t.cos() as f32)
}

/// Filled disk centered at (cx, cy).
fn add_disk(v: &mut Vec<Vertex>, cx: f32, cy: f32, r: f32, color: [f32; 3], seg: usize) {
    for i in 0..seg {
        let a0 = i as f32 / seg as f32 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / seg as f32 * std::f32::consts::TAU;
        let p1 = [cx + r * a0.cos(), cy + r * a0.sin()];
        let p2 = [cx + r * a1.cos(), cy + r * a1.sin()];
        v.push(Vertex { pos: [cx, cy], color });
        v.push(Vertex { pos: p1, color });
        v.push(Vertex { pos: p2, color });
    }
}

/// Annular sector (ring slice) between fractions f0..f1 and radii ri..ro.
fn add_arc(v: &mut Vec<Vertex>, f0: f64, f1: f64, ri: f32, ro: f32, color: [f32; 3], seg: usize) {
    for i in 0..seg {
        let g0 = f0 + (f1 - f0) * (i as f64 / seg as f64);
        let g1 = f0 + (f1 - f0) * ((i + 1) as f64 / seg as f64);
        let (s0x, s0y) = dir(g0);
        let (s1x, s1y) = dir(g1);
        let in0 = [ri * s0x, ri * s0y];
        let out0 = [ro * s0x, ro * s0y];
        let in1 = [ri * s1x, ri * s1y];
        let out1 = [ro * s1x, ro * s1y];
        v.push(Vertex { pos: in0, color });
        v.push(Vertex { pos: out0, color });
        v.push(Vertex { pos: out1, color });
        v.push(Vertex { pos: in0, color });
        v.push(Vertex { pos: out1, color });
        v.push(Vertex { pos: in1, color });
    }
}

/// Thick line segment from a to b with the given half-width.
fn add_seg(v: &mut Vec<Vertex>, a: [f32; 2], b: [f32; 2], half_w: f32, color: [f32; 3]) {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
    let nx = -dy / len * half_w; // perpendicular offset
    let ny = dx / len * half_w;
    let p0 = [a[0] + nx, a[1] + ny];
    let p1 = [a[0] - nx, a[1] - ny];
    let p2 = [b[0] - nx, b[1] - ny];
    let p3 = [b[0] + nx, b[1] + ny];
    v.push(Vertex { pos: p0, color });
    v.push(Vertex { pos: p1, color });
    v.push(Vertex { pos: p2, color });
    v.push(Vertex { pos: p0, color });
    v.push(Vertex { pos: p2, color });
    v.push(Vertex { pos: p3, color });
}

/// A clock hand: a bar from a short tail behind the center out to `length`,
/// pointing along fraction `f` (clockwise from top).
fn add_hand(v: &mut Vec<Vertex>, f: f64, length: f32, back: f32, half_w: f32, color: [f32; 3]) {
    let (sx, sy) = dir(f);
    let tip = [length * sx, length * sy];
    let tail = [-back * sx, -back * sy];
    add_seg(v, tail, tip, half_w, color);
}

/// Minimal uppercase stroke font on a [0,1] x [0,1] cell (origin bottom-left).
/// Each glyph is a list of polylines; only the 14 letters used by the day
/// names are defined. Returns an empty list for anything else.
fn glyph(c: char) -> Vec<Vec<(f32, f32)>> {
    match c {
        'A' => vec![
            vec![(0.0, 0.0), (0.5, 1.0), (1.0, 0.0)],
            vec![(0.18, 0.36), (0.82, 0.36)],
        ],
        'D' => vec![vec![
            (0.0, 0.0), (0.0, 1.0), (0.55, 1.0), (1.0, 0.62), (1.0, 0.38), (0.55, 0.0), (0.0, 0.0),
        ]],
        'E' => vec![
            vec![(1.0, 1.0), (0.0, 1.0), (0.0, 0.0), (1.0, 0.0)],
            vec![(0.0, 0.5), (0.72, 0.5)],
        ],
        'F' => vec![
            vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0)],
            vec![(0.0, 0.5), (0.7, 0.5)],
        ],
        'H' => vec![
            vec![(0.0, 0.0), (0.0, 1.0)],
            vec![(1.0, 0.0), (1.0, 1.0)],
            vec![(0.0, 0.5), (1.0, 0.5)],
        ],
        'I' => vec![
            vec![(0.2, 1.0), (0.8, 1.0)],
            vec![(0.5, 1.0), (0.5, 0.0)],
            vec![(0.2, 0.0), (0.8, 0.0)],
        ],
        'M' => vec![vec![(0.0, 0.0), (0.0, 1.0), (0.5, 0.45), (1.0, 1.0), (1.0, 0.0)]],
        'N' => vec![vec![(0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0)]],
        'O' => vec![vec![
            (0.5, 1.0), (1.0, 0.65), (1.0, 0.35), (0.5, 0.0), (0.0, 0.35), (0.0, 0.65), (0.5, 1.0),
        ]],
        'R' => vec![
            vec![(0.0, 0.0), (0.0, 1.0), (0.7, 1.0), (1.0, 0.78), (0.7, 0.5), (0.0, 0.5)],
            vec![(0.45, 0.5), (1.0, 0.0)],
        ],
        'S' => vec![vec![
            (1.0, 1.0), (0.0, 1.0), (0.0, 0.5), (1.0, 0.5), (1.0, 0.0), (0.0, 0.0),
        ]],
        'T' => vec![
            vec![(0.0, 1.0), (1.0, 1.0)],
            vec![(0.5, 1.0), (0.5, 0.0)],
        ],
        'U' => vec![vec![
            (0.0, 1.0), (0.0, 0.25), (0.3, 0.0), (0.7, 0.0), (1.0, 0.25), (1.0, 1.0),
        ]],
        'W' => vec![vec![(0.0, 1.0), (0.25, 0.0), (0.5, 0.55), (0.75, 0.0), (1.0, 1.0)]],
        '0' => vec![vec![
            (0.25, 1.0), (0.75, 1.0), (1.0, 0.7), (1.0, 0.3), (0.75, 0.0), (0.25, 0.0), (0.0, 0.3),
            (0.0, 0.7), (0.25, 1.0),
        ]],
        '1' => vec![
            vec![(0.25, 0.8), (0.5, 1.0), (0.5, 0.0)],
            vec![(0.2, 0.0), (0.8, 0.0)],
        ],
        '2' => vec![vec![
            (0.0, 1.0), (1.0, 1.0), (1.0, 0.5), (0.0, 0.5), (0.0, 0.0), (1.0, 0.0),
        ]],
        '3' => vec![vec![
            (0.0, 1.0), (1.0, 1.0), (1.0, 0.5), (0.3, 0.5), (1.0, 0.5), (1.0, 0.0), (0.0, 0.0),
        ]],
        '4' => vec![
            vec![(0.0, 1.0), (0.0, 0.45), (1.0, 0.45)],
            vec![(0.7, 1.0), (0.7, 0.0)],
        ],
        '5' => vec![vec![
            (1.0, 1.0), (0.0, 1.0), (0.0, 0.5), (1.0, 0.5), (1.0, 0.0), (0.0, 0.0),
        ]],
        '6' => vec![vec![
            (0.8, 1.0), (0.0, 1.0), (0.0, 0.0), (1.0, 0.0), (1.0, 0.45), (0.0, 0.45),
        ]],
        '7' => vec![vec![(0.0, 1.0), (1.0, 1.0), (0.4, 0.0)]],
        '8' => vec![
            vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)],
            vec![(0.0, 0.5), (1.0, 0.5)],
        ],
        '9' => vec![vec![
            (0.2, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.55), (1.0, 0.55),
        ]],
        _ => vec![],
    }
}

/// Draw `text` centered on (cx, cy) with an arbitrary orientation. `ex` is the
/// unit "advance" direction (text left -> right); `ey` is the unit "up"
/// direction. `h` is the cell height and `stroke` the stroke half-width.
#[allow(clippy::too_many_arguments)]
fn add_text_dir(
    v: &mut Vec<Vertex>,
    text: &str,
    cx: f32,
    cy: f32,
    ex: [f32; 2],
    ey: [f32; 2],
    h: f32,
    stroke: f32,
    color: [f32; 3],
) {
    let cw = h * 0.62; // glyph cell width
    let gap = h * 0.24; // spacing between glyphs
    let n = text.chars().count();
    let total_w = n as f32 * cw + n.saturating_sub(1) as f32 * gap;
    // Map a point in local text space (origin at the word's center) to world.
    let map = |lx: f32, ly: f32| [cx + lx * ex[0] + ly * ey[0], cy + lx * ex[1] + ly * ey[1]];
    let mut adv = -total_w / 2.0; // left edge of the current glyph cell
    for ch in text.chars() {
        for s in glyph(ch) {
            for seg in s.windows(2) {
                let a = map(adv + seg[0].0 * cw, seg[0].1 * h - h / 2.0);
                let b = map(adv + seg[1].0 * cw, seg[1].1 * h - h / 2.0);
                add_seg(v, a, b, stroke, color);
            }
        }
        adv += cw + gap;
    }
}

/// Horizontal text, centered on (cx, cy) — used for the day labels.
fn add_text(v: &mut Vec<Vertex>, text: &str, cx: f32, cy: f32, h: f32, color: [f32; 3]) {
    add_text_dir(v, text, cx, cy, [1.0, 0.0], [0.0, 1.0], h, h * 0.05, color);
}

/// Place `text` reading along a hand that points at fraction `f`, sized to fit
/// inside the hand's width and aligned so its outer edge sits at the hand tip.
fn add_hand_number(
    v: &mut Vec<Vertex>,
    f: f64,
    hand_len: f32,
    text_h: f32,
    stroke: f32,
    color: [f32; 3],
    text: &str,
) {
    let (ux, uy) = dir(f); // unit vector, center -> tip
    let cw = text_h * 0.62;
    let gap = text_h * 0.24;
    let n = text.chars().count();
    let total_w = n as f32 * cw + n.saturating_sub(1) as f32 * gap;
    let margin = text_h * 0.45; // gap between the number and the very tip
    let d = hand_len - margin - total_w / 2.0; // center distance along the hand
    // The number sits centered at distance `d` along the hand regardless of
    // reading direction. Of the two directions that align with the hand, pick
    // the one whose "up" points upward on screen, so the digits stay as upright
    // as possible (never inverted) instead of reading toward the tip blindly.
    // Both choices are proper rotations, so the digits are never mirrored.
    let (ex, ey) = if ux >= 0.0 {
        ([ux, uy], [-uy, ux])
    } else {
        ([-ux, -uy], [uy, -ux])
    };
    add_text_dir(v, text, d * ux, d * uy, ex, ey, text_h, stroke, color);
}

/// Linear interpolation between two RGB colors.
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Color of the numbers on the hands. `weekday` is true Mon-Fri. On weekends,
/// or outside business hours on a weekday, they keep the dark "engraved" color.
/// During business hours on a weekday (9:00-17:00 inclusive) they sweep
/// green -> amber -> red, peaking at amber exactly at noon (the split point,
/// since noon is not the midpoint of the 9am-5pm span).
fn hand_number_color(weekday: bool, tod: f64) -> [f32; 3] {
    const DARK: [f32; 3] = [0.10, 0.11, 0.13];
    const GREEN: [f32; 3] = [0.16, 0.66, 0.24];
    const AMBER: [f32; 3] = [0.96, 0.66, 0.04];
    const RED: [f32; 3] = [0.85, 0.16, 0.14];
    if !weekday || !(9.0..=17.0).contains(&tod) {
        DARK
    } else if tod <= 12.0 {
        lerp3(GREEN, AMBER, ((tod - 9.0) / 3.0) as f32) // 9am green -> noon amber
    } else {
        lerp3(AMBER, RED, ((tod - 12.0) / 5.0) as f32) // noon amber -> 5pm red
    }
}

/// Build the whole clock for the current local time. `sx`/`sy` squash the
/// geometry so the circle stays round regardless of window aspect ratio.
fn build_clock(v: &mut Vec<Vertex>, sx: f32, sy: f32) {
    let now = Local::now();
    let dow = now.weekday().num_days_from_sunday() as f64; // Sun=0 .. Sat=6
    let h_i = now.hour(); // 0..23, shown on the hour hand
    let m_i = now.minute(); // 0..59, shown on the minute hand
    let h = h_i as f64;
    let m = m_i as f64;
    let sec = now.second() as f64 + now.nanosecond() as f64 / 1_000_000_000.0;

    let minute_f = m + sec / 60.0; // 0..60
    let hour_f = h + minute_f / 60.0; // 0..24
    let hour_of_week = dow * 24.0 + hour_f; // 0..168

    let frac_week = hour_of_week / 168.0; // hour hand (1 rev / week)
    let frac_hour = minute_f / 60.0; // minute hand (1 rev / hour)
    let frac_minute = sec / 60.0; // second hand (1 rev / minute)

    // Dark face.
    add_disk(v, 0.0, 0.0, 0.92, [0.08, 0.09, 0.11], 128);

    // Seven colored day arcs forming the bezel (Sun at top, clockwise).
    for (d, &color) in DAY_COLORS.iter().enumerate() {
        add_arc(v, d as f64 / 7.0, (d + 1) as f64 / 7.0, 0.86, 0.96, color, 24);
    }

    // 60 minute/second ticks set just inside the outer edge of the colored
    // bezel, with slightly longer, thicker marks every 5.
    for i in 0..60 {
        let f = i as f64 / 60.0;
        let (dx, dy) = dir(f);
        let major = i % 5 == 0;
        let (inner, w) = if major { (0.918, 0.0026) } else { (0.933, 0.0015) };
        add_seg(v, [0.955 * dx, 0.955 * dy], [inner * dx, inner * dy], w, [0.10, 0.11, 0.13]);
    }

    // 168 hour ticks. Day boundaries (every 24h) are long and white; the 9am-5pm
    // work hours on Mon-Fri are highlighted in green.
    for i in 0..168 {
        let f = i as f64 / 168.0;
        let (dx, dy) = dir(f);
        let day = i / 24; // 0=Sun .. 6=Sat
        let hour = i % 24;
        let day_start = hour == 0;
        let work_hours = (1..=5).contains(&day) && (9..=17).contains(&hour);
        let (inner, w, col) = if day_start {
            (0.73, 0.0055, [1.0, 1.0, 1.0])
        } else if work_hours {
            (0.78, 0.0032, [0.27, 0.85, 0.42]) // Mon-Fri 9am-5pm, green
        } else {
            (0.80, 0.0022, [0.50, 0.54, 0.60])
        };
        add_seg(v, [0.85 * dx, 0.85 * dy], [inner * dx, inner * dy], w, col);
    }

    // Day-name labels, upright, centered in each day's wedge.
    for (d, &name) in DAY_NAMES.iter().enumerate() {
        let f = (d as f64 + 0.5) / 7.0;
        let (dx, dy) = dir(f);
        add_text(v, name, 0.63 * dx, 0.63 * dy, 0.082, [0.93, 0.95, 0.99]);
    }

    // Hands (drawn back-to-front). The hour and minute hands carry a 2-digit
    // readout near their tips, engraved (dark) into the light hand.
    let hour_len = 0.50;
    let min_len = 0.78;
    let weekday = (1.0..=5.0).contains(&dow); // Mon..Fri; Sun(0) and Sat(6) stay dark
    let engrave = hand_number_color(weekday, hour_f);
    add_hand(v, frac_week, hour_len, 0.10, 0.026, [0.93, 0.95, 0.99]); // hour
    add_hand_number(v, frac_week, hour_len, 0.040, 0.0030, engrave, &format!("{:02}", h_i));
    add_hand(v, frac_hour, min_len, 0.12, 0.018, [0.74, 0.80, 0.92]); // minute
    add_hand_number(v, frac_hour, min_len, 0.028, 0.0021, engrave, &format!("{:02}", m_i));
    add_hand(v, frac_minute, 0.82, 0.16, 0.004, [0.96, 0.32, 0.27]); // second

    // Center hub.
    add_disk(v, 0.0, 0.0, 0.030, [0.93, 0.95, 0.99], 32);
    add_disk(v, 0.0, 0.0, 0.014, [0.96, 0.32, 0.27], 24);

    // Apply aspect correction so the clock is always a perfect circle.
    for vert in v.iter_mut() {
        vert.pos[0] *= sx;
        vert.pos[1] *= sy;
    }
}

// ---------------------------------------------------------------------------
// wgpu plumbing.
// ---------------------------------------------------------------------------

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    pipeline: wgpu::RenderPipeline,
    vbuf: wgpu::Buffer,
    vbuf_capacity: u64, // in vertices
    msaa_view: wgpu::TextureView,
    is_fullscreen: bool,
    alpha_mode: wgpu::CompositeAlphaMode, // non-opaque => transparent bg supported
    #[allow(dead_code)] // read only on Windows (window shaping)
    hwnd: isize, // native window handle (0 off-Windows)
}

fn make_msaa(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("msaa"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: SAMPLE_COUNT,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

impl State {
    async fn new(window: Arc<Window>) -> State {
        let size = window.inner_size();

        #[cfg(windows)]
        let hwnd = {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            match window.window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
                _ => 0,
            }
        };
        #[cfg(not(windows))]
        let hwnd = 0isize;

        // winit sets only the small window icon; set the taskbar (big) icon too.
        #[cfg(windows)]
        set_windows_taskbar_icon(hwnd);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });

        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no suitable GPU adapter found");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .expect("failed to create device");

        let caps = surface.get_capabilities(&adapter);
        // Prefer a non-sRGB (linear) format so our colors display as authored.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

        // Transparency strategy is platform-specific:
        //   * Windows: keep an Opaque swapchain and clip the window to the clock
        //     with a region (per-pixel GPU alpha is unreliable on DXGI flip-model
        //     swapchains).
        //   * macOS / Linux: pick a transparency-capable alpha mode so a cleared
        //     (alpha 0) background lets the desktop show through in fullscreen.
        let alpha_mode = if cfg!(windows) {
            wgpu::CompositeAlphaMode::Opaque
        } else if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PostMultiplied)
        {
            wgpu::CompositeAlphaMode::PostMultiplied
        } else if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            caps.alpha_modes[0]
        };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo, // vsync; always supported
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 8,
                            shader_location: 1,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None, // winding varies; don't cull
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: SAMPLE_COUNT,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        let vbuf_capacity: u64 = 16384;
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertices"),
            size: vbuf_capacity * std::mem::size_of::<Vertex>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let msaa_view = make_msaa(&device, &config);

        State {
            window,
            surface,
            device,
            queue,
            config,
            size,
            pipeline,
            vbuf,
            vbuf_capacity,
            msaa_view,
            is_fullscreen: false,
            alpha_mode,
            hwnd,
        }
    }

    fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
            self.msaa_view = make_msaa(&self.device, &self.config);
            self.apply_window_shape();
        }
    }

    /// Toggle borderless fullscreen. Fullscreen drops the title bar/decorations;
    /// the area outside the clock face becomes transparent (via window shaping on
    /// Windows, or a transparent GPU clear on macOS/Linux). Windowed restores the
    /// normal decorated rectangle.
    fn toggle_fullscreen(&mut self) {
        self.is_fullscreen = !self.is_fullscreen;

        // macOS: use *simple* fullscreen (a borderless window covering the screen
        // on the current desktop) rather than native fullscreen, which would move
        // us to a separate Space with an opaque black backdrop and defeat the
        // see-through effect.
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::WindowExtMacOS;
            let _ = self.window.set_simple_fullscreen(self.is_fullscreen);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let mode = if self.is_fullscreen {
                Some(winit::window::Fullscreen::Borderless(None))
            } else {
                None
            };
            self.window.set_fullscreen(mode);
        }

        // A Resized event follows and re-applies the window shape at the new size;
        // do it now too so exiting fullscreen drops the region without a 1-frame lag.
        self.apply_window_shape();
    }

    /// When fullscreen, clip the window to the on-screen clock face so the area
    /// outside it shows the desktop; otherwise restore the full rectangle. A
    /// shaped (non-rectangular) window also forces DWM composition, so the clip
    /// is honored even while fullscreen.
    #[cfg(windows)]
    fn apply_window_shape(&self) {
        let w = self.size.width as i32;
        let h = self.size.height as i32;
        unsafe {
            if self.is_fullscreen {
                // The clock's outer rim sits at 0.96 of the half-min dimension.
                let r = (w.min(h) as f32 * 0.5 * 0.96) as i32;
                let (cx, cy) = (w / 2, h / 2);
                let rgn = CreateEllipticRgn(cx - r, cy - r, cx + r, cy + r);
                SetWindowRgn(self.hwnd, rgn, 1); // takes ownership of `rgn`
            } else {
                SetWindowRgn(self.hwnd, 0, 1); // null region -> normal rectangle
            }
        }
    }

    #[cfg(not(windows))]
    fn apply_window_shape(&self) {}

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let w = self.size.width as f32;
        let h = self.size.height as f32;
        let (sx, sy) = if w >= h { (h / w, 1.0) } else { (1.0, w / h) };

        // In fullscreen with a transparency-capable surface (macOS/Linux), clear
        // to fully transparent so only the opaque clock circle shows and the
        // desktop is visible around it. Otherwise — windowed, or Windows where the
        // window is shaped to the circle instead — clear to the opaque dark
        // background.
        let clear = if self.is_fullscreen && self.alpha_mode != wgpu::CompositeAlphaMode::Opaque {
            wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }
        } else {
            wgpu::Color { r: 0.02, g: 0.02, b: 0.03, a: 1.0 }
        };

        let mut verts: Vec<Vertex> = Vec::with_capacity(self.vbuf_capacity as usize);
        build_clock(&mut verts, sx, sy);
        // Safety valve: never overflow the buffer.
        if verts.len() as u64 > self.vbuf_capacity {
            verts.truncate(self.vbuf_capacity as usize);
        }
        self.queue
            .write_buffer(&self.vbuf, 0, bytemuck::cast_slice(&verts));

        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("encoder") });
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clock pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa_view,
                    resolve_target: Some(&view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&self.pipeline);
            rp.set_vertex_buffer(0, self.vbuf.slice(..));
            rp.draw(0..verts.len() as u32, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        Ok(())
    }
}

/// Decode the embedded PNG into a window icon. This sets the title-bar / taskbar
/// icon on Windows and X11; winit ignores it on macOS and Wayland, which take the
/// icon from the .app bundle / .desktop file instead. Returns None on failure.
fn load_icon() -> Option<winit::window::Icon> {
    let bytes = include_bytes!("../assets/icon.png");
    let mut reader = png::Decoder::new(bytes.as_slice()).read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    winit::window::Icon::from_rgba(buf, info.width, info.height).ok()
}

fn main() {
    let event_loop = EventLoop::new().unwrap();

    // `mut` is only exercised on non-Windows, where the transparency hint is set.
    #[allow(unused_mut)]
    let mut builder = WindowBuilder::new()
        .with_title(format!("168-Hour Week Clock v{}", env!("WEEK_CLOCK_VERSION")))
        .with_window_icon(load_icon())
        .with_inner_size(LogicalSize::new(720.0, 720.0));
    // On macOS/Linux a transparent window lets the desktop show through the
    // cleared (alpha 0) background in fullscreen. On Windows the window stays
    // opaque and is instead shaped to the clock circle (see apply_window_shape).
    #[cfg(not(windows))]
    {
        builder = builder.with_transparent(true);
    }
    // On Windows, start hidden so the taskbar icon (ICON_BIG, set in State::new)
    // is in place before the taskbar button is created when the window is shown.
    #[cfg(windows)]
    {
        builder = builder.with_visible(false);
    }
    let window = Arc::new(builder.build(&event_loop).unwrap());

    let mut state = pollster::block_on(State::new(window));
    state.window.set_visible(true);

    event_loop
        .run(move |event, elwt| {
            elwt.set_control_flow(ControlFlow::Poll);
            match event {
                Event::WindowEvent { window_id, event } if window_id == state.window.id() => {
                    match event {
                        WindowEvent::CloseRequested => elwt.exit(),
                        WindowEvent::Resized(size) => state.resize(size),
                        WindowEvent::KeyboardInput { event: key, .. } => {
                            if key.state == ElementState::Pressed && !key.repeat {
                                match key.physical_key {
                                    PhysicalKey::Code(KeyCode::F11) => state.toggle_fullscreen(),
                                    PhysicalKey::Code(KeyCode::Escape) if state.is_fullscreen => {
                                        state.toggle_fullscreen()
                                    }
                                    _ => {}
                                }
                            }
                        }
                        WindowEvent::RedrawRequested => match state.render() {
                            Ok(()) => {}
                            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                                let s = state.size;
                                state.resize(s);
                            }
                            Err(wgpu::SurfaceError::OutOfMemory) => elwt.exit(),
                            Err(e) => eprintln!("surface error: {e:?}"),
                        },
                        _ => {}
                    }
                }
                Event::AboutToWait => state.window.request_redraw(),
                _ => {}
            }
        })
        .unwrap();
}
