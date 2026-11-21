//! A small floating pill near the bottom of the screen that shows what dictation is doing:
//! listening, transcribing, done. The words sit on top and a waveform runs underneath: bars
//! mirrored around the centre, where the newest level enters and older ones move outwards, so
//! each sound spreads from the middle; they shrink and fade towards the edges, under a glow that
//! follows the voice.
//!
//! macOS: a non-activating NSPanel. Windows: a borderless topmost tool window drawn with GDI.
//! Either way it never takes focus from the app being typed into and ignores the mouse.

use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Hidden,
    /// Shown once at launch, so people know the app is running even if its menu-bar icon is hidden.
    Ready,
    Listening { handsfree: bool },
    Working,
    Done,
    /// Pasting into the focused app isn't possible (Windows: it runs as administrator):
    /// the text is on the clipboard for the user to paste.
    Copied,
    NoSpeech,
    Cancelled,
    Failed,
}

impl Phase {
    /// `key`: the talk key's name; `tap`: press mode "tap to start" (nothing to hold).
    fn label(&self, key: &str, tap: bool) -> String {
        match self {
            Phase::Hidden => String::new(),
            Phase::Ready if tap => format!("PoodPim พร้อมแล้ว · แตะ {key} เพื่อเริ่มพูด"),
            Phase::Ready => format!("PoodPim พร้อมแล้ว · กด {key} ค้างไว้เพื่อพูด"),
            Phase::Listening { handsfree: false } => "กำลังฟัง…".into(),
            Phase::Listening { handsfree: true } => format!("กำลังฟัง… แตะ {key} อีกครั้งเพื่อจบ"),
            Phase::Working => "กำลังถอดเสียง…".into(),
            Phase::Done => "✓ พิมพ์แล้ว".into(),
            Phase::Copied => "คัดลอกไว้แล้ว กด Ctrl+V เอง (แอปนี้รันแบบ Administrator)".into(),
            Phase::NoSpeech => "ไม่ได้ยินเสียงพูด".into(),
            Phase::Cancelled => "ยกเลิกแล้ว".into(),
            Phase::Failed => "ถอดเสียงไม่สำเร็จ (ดูในเมนู)".into(),
        }
    }

    /// How long a final phase stays on screen.
    fn linger(&self) -> Option<Duration> {
        match self {
            Phase::Done => Some(Duration::from_millis(900)),
            Phase::NoSpeech | Phase::Cancelled => Some(Duration::from_millis(1200)),
            Phase::Failed | Phase::Copied => Some(Duration::from_millis(3500)),
            Phase::Ready => Some(Duration::from_millis(3000)),
            _ => None,
        }
    }

    fn rgb(&self) -> [f64; 3] {
        match self {
            Phase::Listening { .. } => [1.0, 1.0, 1.0],
            Phase::Working => [1.0, 0.70, 0.25],
            Phase::Done | Phase::Ready => [0.20, 0.84, 0.29],
            Phase::Copied => [1.0, 0.70, 0.25],
            Phase::Failed => [1.0, 0.27, 0.23],
            _ => [0.6, 0.6, 0.62],
        }
    }
}

pub struct Overlay {
    phase: Phase,
    /// While listening: the end of what has been said so far, shown instead of the label.
    preview: String,
    /// The talk key's name and whether it is tapped to start (press mode), for the labels.
    talk_key: (String, bool),
    since: Instant,
    level: f32,
    shown: f32,
    /// What the waveform shows, newest first: one entry per bar from the centre outwards.
    history: [f32; BARS_PER_SIDE + 1],
    pushed: Instant,
    /// Bar heights on screen (0..1), easing towards their targets between steps.
    bars: [f32; BARS],
    view: Option<sys::View>,
}

/// How much of the text being said the pill is given (its end, which is what changes). More than
/// fits: the pill shows the end and fades out the start at its left edge.
const PREVIEW_CHARS: usize = 64;

/// The last PREVIEW_CHARS characters of `text`; never starting on a Thai vowel sign or tone mark,
/// which belong to the letter before them.
fn preview_tail(text: &str) -> String {
    let text = text.trim();
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= PREVIEW_CHARS {
        return text.to_string();
    }
    let mut from = chars.len() - PREVIEW_CHARS;
    while from < chars.len() && matches!(chars[from], '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}') {
        from += 1;
    }
    chars[from..].iter().collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn preview_shows_the_end_without_a_loose_mark() {
        assert_eq!(super::preview_tail(" สวัสดีครับ "), "สวัสดีครับ");
        // 66 chars: the cut falls on ่ (the 3rd), which must not start the tail.
        let long = format!("ที่{}", "ก".repeat(63));
        assert_eq!(super::preview_tail(&long), "ก".repeat(63));
    }
}

#[cfg(target_os = "macos")]
use mac as sys;
#[cfg(windows)]
use win as sys;

const BARS_PER_SIDE: usize = 11;
const BARS: usize = 2 * BARS_PER_SIDE + 1;
/// How often the waveform moves one bar outwards.
const STEP: Duration = Duration::from_millis(50);

/// 1 at the centre bar, falling linearly towards the edges.
fn falloff(bar: usize) -> f32 {
    1.0 - bar.abs_diff(BARS_PER_SIDE) as f32 / (BARS_PER_SIDE + 1) as f32
}

/// How opaque bar `bar` is: the edges fade down to faint dots.
fn bar_alpha(bar: usize) -> f64 {
    let f = falloff(bar) as f64;
    0.2 + 0.8 * f * f
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl Overlay {
    pub fn new() -> Self {
        Self {
            phase: Phase::Hidden,
            preview: String::new(),
            talk_key: (crate::platform::TALK_KEY_LABEL.into(), false),
            since: Instant::now(),
            level: 0.0,
            shown: 0.0,
            history: [0.0; BARS_PER_SIDE + 1],
            pushed: Instant::now(),
            bars: [0.0; BARS],
            view: None,
        }
    }

    pub fn set_talk_key(&mut self, key: &str, tap: bool) {
        self.talk_key = (key.to_string(), tap);
    }

    fn label(&self) -> String {
        self.phase.label(&self.talk_key.0, self.talk_key.1)
    }

    pub fn set(&mut self, phase: Phase) {
        if phase == self.phase {
            return;
        }
        if self.phase == Phase::Hidden {
            self.history = [0.0; BARS_PER_SIDE + 1];
            self.bars = [0.0; BARS];
        }
        self.phase = phase;
        self.preview.clear();
        self.since = Instant::now();
        if self.view.is_none() && self.phase != Phase::Hidden {
            self.view = sys::View::new();
        }
        if let Some(v) = &self.view {
            if self.phase == Phase::Hidden {
                v.hide();
            } else {
                v.show(&self.label(), self.phase.rgb());
                if !v.on_active_space() {
                    // A new panel joins the Spaces there are now (see View::on_active_space).
                    eprintln!("overlay: not on the current Space, made again");
                    self.view = sys::View::new();
                    if let Some(v) = &self.view {
                        v.show(&self.label(), self.phase.rgb());
                    }
                }
            }
        }
        self.tick();
    }

    /// While listening, shows the end of `text` (what has been said so far, not yet typed)
    /// in place of the label; empty brings the label back.
    pub fn set_preview(&mut self, text: &str) {
        let tail = preview_tail(text);
        if !matches!(self.phase, Phase::Listening { .. }) || tail == self.preview {
            return;
        }
        self.preview = tail;
        if let Some(v) = &self.view {
            let label = if self.preview.is_empty() { self.label() } else { self.preview.clone() };
            v.show(&label, self.phase.rgb());
        }
    }

    /// Microphone level of the last ~50 ms (RMS of samples in -1..1).
    pub fn set_level(&mut self, rms: f32) {
        let db = 20.0 * (rms + 1e-9).log10();
        self.level = ((db + 55.0) / 40.0).clamp(0.0, 1.0);
    }

    /// Advances the animation. Returns true while it should be called again soon (~30 fps).
    pub fn tick(&mut self) -> bool {
        if let Some(d) = self.phase.linger() {
            if self.since.elapsed() >= d {
                self.set(Phase::Hidden);
            }
        }
        if self.phase == Phase::Hidden {
            return false;
        }
        // Fast attack, slower release, like a VU meter.
        let k = if self.level > self.shown { 0.6 } else { 0.2 };
        self.shown += (self.level - self.shown) * k;
        if self.pushed.elapsed() >= STEP {
            self.pushed = Instant::now();
            let t = self.since.elapsed().as_secs_f32();
            let breath = |period: f32, base: f32, depth: f32| base + depth * (t * std::f32::consts::TAU / period).sin();
            let next = match self.phase {
                Phase::Listening { .. } => self.shown,
                // A sine fed through the same buffer ripples outwards from the centre.
                Phase::Working => breath(1.1, 0.35, 0.25),
                Phase::Ready => breath(3.2, 0.3, 0.2),
                _ => 0.0,
            };
            self.history.rotate_right(1);
            self.history[0] = next;
        }
        for (i, h) in self.bars.iter_mut().enumerate() {
            let f = falloff(i);
            let target = self.history[i.abs_diff(BARS_PER_SIDE)] * f * f;
            *h += (target - *h) * 0.5;
        }
        if let Some(v) = &self.view {
            v.set_bars(&self.bars, self.history[0]);
        }
        true
    }
}

#[cfg(target_os = "macos")]
#[allow(unused_unsafe)]
mod mac {
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSBackingStoreType, NSBox, NSBoxType, NSColor, NSFont, NSFontWeightMedium, NSPanel, NSScreen, NSStatusWindowLevel,
        NSTextField, NSTitlePosition, NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSArray, NSNumber, NSPoint, NSRect, NSSize, NSString};
    use objc2_quartz_core::CAGradientLayer;

    use super::{BARS, bar_alpha};

    const RADIUS: f64 = 22.0;
    const BAR_W: f64 = 3.5;
    const BAR_GAP: f64 = 3.0;
    /// How much taller than a dot the loudest centre bar gets.
    const MAX_RISE: f64 = 24.0;
    const PAD: f64 = 22.0;
    /// The words get this much width whatever they say; longer text shows its end and fades out
    /// at the edges.
    const TEXT_W: f64 = 340.0;
    const FADE: f64 = 30.0;
    /// Sukhumvit Set has no loops (หัว); the system font falls back to Thonburi for Thai, which does.
    const FONT: &str = "SukhumvitSet-Medium";
    const FONT_SIZE: f64 = 15.0;
    /// The waveform's centre line, from the bottom of the pill.
    const WAVE_Y: f64 = 26.0;
    /// The words' bottom, from the bottom of the pill: clear of the loudest bars.
    const TEXT_Y: f64 = 52.0;
    const TOP_PAD: f64 = 12.0;
    /// Rings of the glow, innermost last; each adds a little light, so the centre is brightest.
    const GLOW: [(f64, f64); 5] = [(150.0, 40.0), (118.0, 34.0), (88.0, 28.0), (60.0, 22.0), (34.0, 16.0)];
    const GLOW_ALPHA: f64 = 0.085;
    /// Distance from the bottom of the usable screen area (above the Dock).
    const BOTTOM: f64 = 64.0;

    pub struct View {
        mtm: MainThreadMarker,
        panel: Retained<NSPanel>,
        bg: Retained<NSBox>,
        glow: Vec<Retained<NSBox>>,
        bars: Vec<Retained<NSBox>>,
        /// Clips the label to TEXT_W and fades it out at both ends (a gradient mask on its layer).
        text_box: Retained<NSView>,
        label: Retained<NSTextField>,
    }

    fn wave_width() -> f64 {
        BARS as f64 * BAR_W + (BARS - 1) as f64 * BAR_GAP
    }

    const W: f64 = PAD + TEXT_W + PAD;

    /// Opaque in the middle, clear at both ends: as a mask, it fades what it covers out at the sides.
    fn fade_mask() -> Retained<CAGradientLayer> {
        let mask = CAGradientLayer::new();
        let clear = unsafe { NSColor::colorWithWhite_alpha(0.0, 0.0) }.CGColor();
        let solid = unsafe { NSColor::colorWithWhite_alpha(0.0, 1.0) }.CGColor();
        // CGColor is toll-free bridged: an object CAGradientLayer's `colors` array can hold.
        fn obj<T>(c: &T) -> &AnyObject {
            unsafe { &*(c as *const T as *const AnyObject) }
        }
        let colors = NSArray::from_slice(&[obj(&*clear), obj(&*solid), obj(&*solid), obj(&*clear)]);
        let f = FADE / TEXT_W;
        let stops = [0.0, f, 1.0 - f, 1.0].map(NSNumber::numberWithDouble);
        unsafe { mask.setColors(Some(&colors)) };
        mask.setLocations(Some(&NSArray::from_retained_slice(&stops)));
        mask.setStartPoint(NSPoint::new(0.0, 0.5));
        mask.setEndPoint(NSPoint::new(1.0, 0.5));
        mask
    }

    fn color(rgb: [f64; 3], a: f64) -> Retained<NSColor> {
        unsafe { NSColor::colorWithSRGBRed_green_blue_alpha(rgb[0], rgb[1], rgb[2], a) }
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    fn filled_box(mtm: MainThreadMarker, frame: NSRect, fill: &NSColor, radius: f64) -> Retained<NSBox> {
        unsafe {
            let b = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
            b.setBoxType(NSBoxType::Custom);
            b.setTitlePosition(NSTitlePosition::NoTitle);
            b.setBorderWidth(0.0);
            b.setBorderColor(&NSColor::clearColor());
            b.setContentViewMargins(NSSize::new(0.0, 0.0));
            b.setFillColor(fill);
            b.setCornerRadius(radius);
            b
        }
    }

    impl View {
        pub fn new() -> Option<Self> {
            let mtm = MainThreadMarker::new()?;
            unsafe {
                let frame = rect(0.0, 0.0, W, 80.0);
                let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
                    NSPanel::alloc(mtm),
                    frame,
                    NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                    NSBackingStoreType::Buffered,
                    false,
                );
                panel.setReleasedWhenClosed(false);
                panel.setOpaque(false);
                panel.setBackgroundColor(Some(&NSColor::clearColor()));
                panel.setHasShadow(true);
                panel.setIgnoresMouseEvents(true);
                panel.setHidesOnDeactivate(false);
                panel.setFloatingPanel(true);
                panel.setBecomesKeyOnlyIfNeeded(true);
                panel.setLevel(NSStatusWindowLevel);
                panel.setCollectionBehavior(
                    NSWindowCollectionBehavior::CanJoinAllSpaces
                        | NSWindowCollectionBehavior::FullScreenAuxiliary
                        | NSWindowCollectionBehavior::Stationary
                        | NSWindowCollectionBehavior::IgnoresCycle,
                );
                let content = panel.contentView()?;
                let bg = filled_box(mtm, frame, &color([0.09, 0.09, 0.10], 0.94), RADIUS);
                bg.setBorderWidth(1.0);
                bg.setBorderColor(&color([1.0, 1.0, 1.0], 0.12));
                content.addSubview(&bg);
                let glow: Vec<_> = GLOW
                    .iter()
                    .map(|&(w, h)| {
                        let b = filled_box(mtm, rect(0.0, 0.0, w, h), &color([1.0, 1.0, 1.0], GLOW_ALPHA), h / 2.0);
                        b.setAlphaValue(0.0);
                        content.addSubview(&b);
                        b
                    })
                    .collect();
                let bars: Vec<_> = (0..BARS)
                    .map(|_| {
                        let b = filled_box(mtm, rect(0.0, 0.0, BAR_W, BAR_W), &color([1.0, 1.0, 1.0], 1.0), BAR_W / 2.0);
                        content.addSubview(&b);
                        b
                    })
                    .collect();
                let text_box = NSView::initWithFrame(NSView::alloc(mtm), rect(PAD, TEXT_Y, TEXT_W, 20.0));
                text_box.setWantsLayer(true);
                if let Some(layer) = text_box.layer() {
                    layer.setMask(Some(&fade_mask()));
                }
                content.addSubview(&text_box);
                let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
                label.setTextColor(Some(&NSColor::whiteColor()));
                let font = NSFont::fontWithName_size(&NSString::from_str(FONT), FONT_SIZE)
                    .unwrap_or_else(|| NSFont::systemFontOfSize_weight(FONT_SIZE, NSFontWeightMedium));
                label.setFont(Some(&font));
                text_box.addSubview(&label);
                Some(Self { mtm, panel, bg, glow, bars, text_box, label })
            }
        }

        pub fn show(&self, text: &str, rgb: [f64; 3]) {
            unsafe {
                self.label.setStringValue(&NSString::from_str(text));
                let fit = self.label.fittingSize();
                let (tw, th) = (fit.width + 4.0, fit.height);
                // Centred while it fits between the fades; past that, its end (the newest words) shows.
                let x = if tw <= TEXT_W - 2.0 * FADE { (TEXT_W - tw) / 2.0 } else { TEXT_W - FADE / 2.0 - tw };
                self.label.setFrame(rect(x.round(), 0.0, tw, th));
                self.text_box.setFrame(rect(PAD, TEXT_Y, TEXT_W, th));
                if let Some(mask) = self.text_box.layer().and_then(|l| l.mask()) {
                    mask.setFrame(rect(0.0, 0.0, TEXT_W, th));
                }
                let (w, h) = (W, (TEXT_Y + th + TOP_PAD).ceil());
                self.bg.setFrame(rect(0.0, 0.0, w, h));
                for (i, b) in self.bars.iter().enumerate() {
                    b.setFillColor(&color(rgb, bar_alpha(i)));
                }
                for (b, &(gw, gh)) in self.glow.iter().zip(&GLOW) {
                    b.setFillColor(&color(rgb, GLOW_ALPHA));
                    b.setFrame(rect(((w - gw) / 2.0).round(), (WAVE_Y - gh / 2.0).round(), gw, gh));
                }
                // On the screen the user is working on, centred above the Dock.
                let screen = NSScreen::mainScreen(self.mtm).or_else(|| NSScreen::screens(self.mtm).firstObject());
                let (x, y) = match screen {
                    Some(s) => {
                        let vf = s.visibleFrame();
                        (vf.origin.x + (vf.size.width - w) / 2.0, vf.origin.y + BOTTOM)
                    }
                    None => (100.0, 100.0),
                };
                self.panel.setFrame_display(rect(x.round(), y.round(), w, h), true);
                self.panel.orderFrontRegardless();
                self.panel.invalidateShadow();
            }
        }

        /// Bar heights as fractions (0..1) of the maximum; `glow`: how loud the newest sound is (0..1).
        pub fn set_bars(&self, heights: &[f32; BARS], glow: f32) {
            let left = (W - wave_width()) / 2.0;
            for (i, (b, &h)) in self.bars.iter().zip(heights).enumerate() {
                let bh = BAR_W + MAX_RISE * h as f64;
                let x = left + i as f64 * (BAR_W + BAR_GAP);
                unsafe { b.setFrame(rect(x, WAVE_Y - bh / 2.0, BAR_W, bh)) };
            }
            for g in &self.glow {
                unsafe { g.setAlphaValue(glow.clamp(0.0, 1.0) as f64) };
            }
        }

        pub fn hide(&self) {
            unsafe { self.panel.orderOut(None) };
        }

        /// Whether the panel is on the Space the user is looking at. A panel made to join all
        /// Spaces was seen missing from a full-screen Space (a Zoom meeting) that, it seems,
        /// came after it: it stayed on the first desktop only.
        pub fn on_active_space(&self) -> bool {
            unsafe { self.panel.isOnActiveSpace() }
        }
    }

    impl Drop for View {
        fn drop(&mut self) {
            unsafe {
                self.panel.orderOut(None);
                self.panel.close();
            }
        }
    }
}

/// Windows: a WS_POPUP tool window (no taskbar button), topmost, click-through, never
/// activated, rounded with a window region and drawn with GDI into a back buffer.
/// Created and used on the event-loop thread, whose message pump delivers WM_PAINT.
#[cfg(windows)]
mod win {
    use std::cell::RefCell;

    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, BitBlt, CLEARTYPE_QUALITY, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontIndirectW,
        CreateRoundRectRgn, CreateSolidBrush, DEFAULT_CHARSET, DeleteDC, DeleteObject, ETO_CLIPPED, EndPaint, ExtTextOutW,
        FW_MEDIUM, FillRect,
        GetDC, GetMonitorInfoW, GetStockObject, GetTextExtentPoint32W, HFONT, InvalidateRect, LOGFONTW,
        MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromWindow, NULL_PEN, PAINTSTRUCT, ReleaseDC, RoundRect, SRCCOPY,
        SelectObject, SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, GetForegroundWindow, HTTRANSPARENT, HWND_TOPMOST, LWA_ALPHA, MA_NOACTIVATE,
        RegisterClassExW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetLayeredWindowAttributes, SetWindowPos,
        ShowWindow, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_PAINT, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };
    use windows::core::{PCWSTR, w};

    use super::{BARS, bar_alpha};

    // In pixels at 100% scaling (96 DPI), measured from the top (the macOS view's from the bottom).
    const H: f32 = 86.0;
    const RADIUS: f32 = 22.0;
    const BAR_W: f32 = 3.5;
    const BAR_GAP: f32 = 3.0;
    const MAX_RISE: f32 = 24.0;
    const PAD: f32 = 22.0;
    /// The words get this much width whatever they say; longer text shows its end, clipped.
    const TEXT_W: f32 = 340.0;
    /// The waveform's centre line, from the top.
    const WAVE_Y: f32 = 60.0;
    /// The label's top.
    const TEXT_Y: f32 = 12.0;
    /// Rings of the glow, outermost first; each adds a little light, so the centre is brightest.
    const GLOW: [(f32, f32); 5] = [(150.0, 40.0), (118.0, 34.0), (88.0, 28.0), (60.0, 22.0), (34.0, 16.0)];
    const GLOW_ALPHA: f64 = 0.085;
    const BG: [f64; 3] = [0.09, 0.09, 0.10];

    fn wave_width() -> f32 {
        BARS as f32 * BAR_W + (BARS - 1) as f32 * BAR_GAP
    }

    /// `rgb` at opacity `a` over the background (the window is opaque, so blending is done here).
    fn over_bg(rgb: [f64; 3], a: f64) -> [f64; 3] {
        [0, 1, 2].map(|i| BG[i] + (rgb[i] - BG[i]) * a)
    }
    /// Distance from the bottom of the work area (above the taskbar).
    const BOTTOM: f32 = 64.0;

    /// What WM_PAINT draws (one overlay per app, on one thread). Sizes are in pixels at
    /// `scale` (the monitor's DPI / 96: tao makes the process DPI-aware, so nothing is scaled
    /// for us).
    struct Look {
        text: Vec<u16>,
        rgb: [f64; 3],
        bars: [f32; BARS],
        glow: f32,
        width: i32,
        scale: f32,
        font: HFONT,
    }

    impl Look {
        fn px(&self, v: f32) -> i32 {
            (v * self.scale).round() as i32
        }
    }

    fn make_font(scale: f32) -> HFONT {
        // Leelawadee UI ships with Windows 10/11 and covers both Thai and Latin.
        let mut lf = LOGFONTW {
            lfHeight: -(18.0 * scale).round() as i32,
            lfWeight: FW_MEDIUM.0 as i32,
            lfCharSet: DEFAULT_CHARSET,
            lfQuality: CLEARTYPE_QUALITY,
            ..Default::default()
        };
        for (i, c) in "Leelawadee UI".encode_utf16().enumerate() {
            lf.lfFaceName[i] = c;
        }
        unsafe { CreateFontIndirectW(&lf) }
    }

    thread_local! {
        static LOOK: RefCell<Option<Look>> = const { RefCell::new(None) };
    }

    fn colorref(rgb: [f64; 3]) -> COLORREF {
        let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        COLORREF(c(rgb[0]) | (c(rgb[1]) << 8) | (c(rgb[2]) << 16))
    }

    pub struct View {
        hwnd: HWND,
    }

    impl View {
        pub fn new() -> Option<Self> {
            unsafe {
                let instance = GetModuleHandleW(None).ok()?;
                let class = w!("PoodPimOverlay");
                let wc = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(wnd_proc),
                    hInstance: instance.into(),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassExW(&wc); // fails harmlessly if already registered
                let hwnd = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
                    class,
                    w!("PoodPim"),
                    WS_POPUP,
                    0,
                    0,
                    200,
                    H as i32,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .ok()?;
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 235, LWA_ALPHA);
                let font = make_font(1.0);
                LOOK.with(|l| {
                    *l.borrow_mut() = Some(Look { text: Vec::new(), rgb: [1.0; 3], bars: [0.0; BARS], glow: 0.0, width: 200, scale: 1.0, font })
                });
                Some(Self { hwnd })
            }
        }

        pub fn show(&self, text: &str, rgb: [f64; 3]) {
            let text: Vec<u16> = text.encode_utf16().collect();
            // The screen the user is working on: move there first, so the DPI read below is
            // that monitor's.
            let work = unsafe {
                let monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
                let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                GetMonitorInfoW(monitor, &mut info).as_bool().then_some(info.rcWork)
            };
            if let Some(r) = work {
                unsafe {
                    let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), r.left, r.top, 1, 1, SWP_NOACTIVATE);
                }
            }
            let scale = match unsafe { GetDpiForWindow(self.hwnd) } {
                0 => 1.0,
                dpi => dpi as f32 / 96.0,
            };
            let size = LOOK.with(|l| {
                let mut l = l.borrow_mut();
                let look = l.as_mut()?;
                if (look.scale - scale).abs() > 0.01 {
                    let old = std::mem::replace(&mut look.font, make_font(scale));
                    unsafe {
                        let _ = DeleteObject(old.into());
                    }
                    look.scale = scale;
                }
                let mut size = SIZE::default();
                unsafe {
                    let dc = GetDC(Some(self.hwnd));
                    let old = SelectObject(dc, look.font.into());
                    let _ = GetTextExtentPoint32W(dc, &text, &mut size);
                    SelectObject(dc, old);
                    ReleaseDC(Some(self.hwnd), dc);
                }
                look.width = look.px(PAD + TEXT_W + PAD);
                look.text = text;
                look.rgb = rgb;
                Some((look.width, look.px(H), look.px(BOTTOM)))
            });
            let Some((w, h, bottom)) = size else { return };
            unsafe {
                // Bottom centre of that screen, above the taskbar.
                let (x, y) = match work {
                    Some(r) => (r.left + (r.right - r.left - w) / 2, r.bottom - bottom - h),
                    None => (100, 100),
                };
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
                let r = (RADIUS * 2.0 * scale).round() as i32;
                let region = CreateRoundRectRgn(0, 0, w + 1, h + 1, r, r);
                SetWindowRgn(self.hwnd, Some(region), true); // the window owns the region now
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }

        /// Bar heights as fractions (0..1) of the maximum.
        pub fn set_bars(&self, heights: &[f32; BARS], glow: f32) {
            LOOK.with(|l| {
                if let Some(look) = l.borrow_mut().as_mut() {
                    look.bars = *heights;
                    look.glow = glow.clamp(0.0, 1.0);
                }
            });
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }

        pub fn hide(&self) {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }

        /// Windows has no Spaces: a topmost window is on every virtual desktop's screen.
        pub fn on_active_space(&self) -> bool {
            true
        }
    }

    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_PAINT => {
                unsafe { paint(hwnd) };
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    unsafe fn paint(hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let dc = unsafe { BeginPaint(hwnd, &mut ps) };
        LOOK.with(|l| {
            let l = l.borrow();
            let Some(look) = l.as_ref() else { return };
            let (w, h) = (look.width, look.px(H));
            unsafe {
                // Draw into a back buffer, then copy: no flicker at 30 frames a second.
                let mem = CreateCompatibleDC(Some(dc));
                let bmp = CreateCompatibleBitmap(dc, w, h);
                let old_bmp = SelectObject(mem, bmp.into());
                let bg = CreateSolidBrush(colorref(BG));
                FillRect(mem, &RECT { left: 0, top: 0, right: w, bottom: h }, bg);
                let _ = DeleteObject(bg.into());
                let old_pen = SelectObject(mem, GetStockObject(NULL_PEN));
                // Rounded shapes filled with `rgb` blended over the background.
                let shape = |rgb: [f64; 3], left: i32, top: i32, width: i32, height: i32| {
                    let brush = CreateSolidBrush(colorref(rgb));
                    let old = SelectObject(mem, brush.into());
                    let r = width.min(height);
                    let _ = RoundRect(mem, left, top, left + width + 1, top + height + 1, r, r);
                    SelectObject(mem, old);
                    let _ = DeleteObject(brush.into());
                };
                let wave_y = look.px(WAVE_Y);
                for (ring, &(gw, gh)) in GLOW.iter().enumerate() {
                    let light = GLOW_ALPHA * look.glow as f64 * (ring + 1) as f64;
                    let (gw, gh) = (look.px(gw), look.px(gh));
                    shape(over_bg(look.rgb, light), (w - gw) / 2, wave_y - gh / 2, gw, gh);
                }
                let left = (w - look.px(wave_width())) / 2;
                let bar_w = look.px(BAR_W);
                for (i, &level) in look.bars.iter().enumerate() {
                    let bh = look.px(BAR_W + MAX_RISE * level);
                    let x = left + look.px(i as f32 * (BAR_W + BAR_GAP));
                    shape(over_bg(look.rgb, bar_alpha(i)), x, wave_y - bh / 2, bar_w, bh);
                }
                SelectObject(mem, old_pen);
                let old_font = SelectObject(mem, look.font.into());
                SetBkMode(mem, TRANSPARENT);
                SetTextColor(mem, COLORREF(0x00FF_FFFF));
                let mut size = SIZE::default();
                let _ = GetTextExtentPoint32W(mem, &look.text, &mut size);
                let (pad, text_w) = (look.px(PAD), look.px(TEXT_W));
                let x = if size.cx <= text_w { (w - size.cx) / 2 } else { pad + text_w - size.cx };
                let clip = RECT { left: pad, top: 0, right: pad + text_w, bottom: h };
                let _ = ExtTextOutW(mem, x, look.px(TEXT_Y), ETO_CLIPPED, Some(&clip), PCWSTR(look.text.as_ptr()), look.text.len() as u32, None);
                SelectObject(mem, old_font);
                let _ = BitBlt(dc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
                SelectObject(mem, old_bmp);
                let _ = DeleteObject(bmp.into());
                let _ = DeleteDC(mem);
            }
        });
        unsafe {
            let _ = EndPaint(hwnd, &ps);
        }
    }
}
