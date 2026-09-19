# Terminal Rendering for a 10-Band Retro Hi-Fi Spectrum Analyzer

## Which Unicode or ASCII glyphs best mimic separated LED/VFD segments across common terminals?

### Takeaway
Use a custom Ratatui widget whose normal Unicode segment is `▄` (U+2584 LOWER HALF BLOCK), repeated across each band and colored per row. Provide an explicit ASCII mode using `=` for segments and `-` for peaks; do not depend on geometric shapes, emoji, private-use characters, combining characters, or a custom font.

### Cited Findings
- Ratatui's built-in vertical `BarChart` defaults to the `NINE_LEVELS` bar symbol set, and its documented output uses block-element glyphs. It exposes `bar_width`, `bar_gap`, `bar_set`, per-bar styling, and a fixed `max`, but its bars are continuous rather than independently styled segment rows. [Source](https://docs.rs/ratatui/latest/ratatui/widgets/struct.BarChart.html)
- Ratatui's `Sparkline` likewise defaults to `NINE_LEVELS`, permits `THREE_LEVELS`, `NINE_LEVELS`, or a custom symbol set, and supports styles on individual bars. This confirms that block elements are an established Ratatui convention, although a sparkline is not the right geometry for ten multi-column segmented bands. [Source](https://docs.rs/ratatui/latest/ratatui/widgets/struct.Sparkline.html)
- A Ratatui `Buffer` is a grid in which each cell contains a grapheme, foreground color, and background color. `cell_mut` permits safe cell-by-cell symbol and style changes, and `diff_iter` builds the minimal changed-cell sequence for the next frame. [Source](https://docs.rs/ratatui/latest/ratatui/buffer/struct.Buffer.html)
- The `unicode-width` crate computes terminal column widths from Unicode width rules and offers separate normal and CJK calculations; ambiguous characters may be one column outside East Asian contexts and two in East Asian contexts. [Source](https://docs.rs/unicode-width/latest/unicode_width/)
- Unicode explicitly says East Asian Width is not an off-the-shelf width solution for modern terminal emulators and that ambiguous characters need contextual resolution; ASCII is classified East Asian Narrow. [Source](https://www.unicode.org/reports/tr11/)
- CAVA warns that some fonts have unusual or missing glyphs for non-bottom orientations. Its terminal visualizer therefore supplies practical evidence that glyph and font behavior cannot be assumed even when a character is valid Unicode. [Source](https://raw.githubusercontent.com/karlstav/cava/master/example_files/config)

### Inferences
- **Primary glyph:** render each lit or unlit segment as `▄`. Its filled lower half creates visible black space above every segment, so adjacent terminal rows read as discrete VFD/LED elements without sacrificing an entire blank row. Repeat it across the band's width, such as `▄▄▄▄▄` at 72 columns. `█` is the safest dense alternative but reads as a continuous bar; `▁` is thinner and can disappear at small font sizes.
- **Fallback glyphs:** expose `glyph_mode = unicode | ascii`, defaulting to Unicode. ASCII mode should use `=====` for an on segment, a dim `.....` or spaces for off segments, and `---` for a peak marker. ASCII has the strongest width guarantee because all ASCII is narrow under Unicode's width classification.
- **Avoid:** `■`, `●`, `▬`, Braille patterns, emoji, and private-use icons. Their apparent size, baseline, advance width, or font coverage varies more than the block elements, while none improves the ten-band model enough to justify the risk.
- **Validate, but do not over-trust, widths:** assert at startup or in tests that every configured meter glyph has `UnicodeWidthStr::width(glyph) == 1` and reject zero- or two-column custom glyphs. Keep ASCII mode because a library width calculation cannot guarantee that every emulator and font renders an ambiguous glyph identically.
- **Peak glyph:** use a short centered `-` run even in Unicode mode, styled bright white or bright yellow. It is unmistakably separate from `▄`, occupies one cell per character, and degrades cleanly. If the marker coincides with the live top segment, draw the live segment first and the peak marker second.
- **Rendering primitive:** implement `Widget::render(area, buf)` directly and update cells through `buf.cell_mut((x, y))`. A custom cell renderer is simpler and more controllable than forcing row-dependent color zones, off-segments, and peak markers through `BarChart`.
- **Accessibility/debug option:** include a monochrome palette in which lit segments are `=`/`▄`, off segments are spaces or `.`, and peaks are `-`; color must reinforce state, not be its only carrier.

### Gaps
- No Unicode specification guarantees identical glyph artwork or one-cell behavior in every terminal/font pairing. The recommendation therefore relies on conservative block elements plus a user-selectable ASCII fallback rather than claiming universal Unicode rendering.

## How should columns, gutters, peak-hold markers, labels and color zones adapt to narrow and wide layouts?

### Takeaway
Keep all ten bands visible and independent at every supported width. At the 72-column floor, use five-cell bands, one-cell gutters, compact frequency labels, and a four-cell dB scale; add gutter and band width only at deterministic breakpoints, then cap band width so very wide terminals do not turn the analyzer into ten featureless slabs.

### Cited Findings
- Ratatui's `BarChart` defaults both `bar_width` and `bar_gap` to one, permits explicit widths and gaps, and constrains a label to the bar's width. [Source](https://docs.rs/ratatui/latest/ratatui/widgets/struct.BarChart.html)
- Ratatui allows individual bar styles and a fixed maximum. A fixed maximum is important for a meter because omitting it makes the largest current datum become the chart maximum, visually rescaling every frame. [Source](https://docs.rs/ratatui/latest/ratatui/widgets/struct.BarChart.html)
- Ratatui supports the 16 named ANSI colors plus indexed and RGB colors. Its documentation warns that RGB output is only reliable on terminals with 24-bit color support and can be unpredictable with some backends or terminals when unsupported. [Source](https://docs.rs/ratatui/latest/ratatui/style/enum.Color.html)
- CAVA's reference terminal configuration defaults to two-character bars with one-character spacing, supports centering, frequency-axis labels, vertical gradients, and a 60 FPS target. Its noncurses output uses buffering and cursor movement to print only frame-to-frame changes, which it describes as less resource-intensive and less prone to tearing. [Source](https://raw.githubusercontent.com/karlstav/cava/master/example_files/config)
- Ratatui buffers similarly expose a minimal changed-cell diff rather than requiring direct full-screen output from each widget. [Source](https://docs.rs/ratatui/latest/ratatui/buffer/struct.Buffer.html)

### Inferences
- **Deterministic width budget:** after an optional one-cell border, reserve four columns for the dB scale (`"  0 "`, `"-12 "`, etc.). Let `plot_width = area.width - 2 - 4`. At width 72 this leaves 66 plot columns. Ten bands of width 5 plus nine one-cell gutters consume 59 columns, leaving seven columns to split as centered left/right padding. This is robust at the exact required minimum without dropping a band.
- **Horizontal breakpoints:** use one-cell gutters for widths 72-91 and two-cell gutters at 92 and above. Compute `band_width = min(7, floor((plot_width - 9 * gutter) / 10))`; center the used width and distribute any odd remainder to the right. This yields five-cell bands at 72, grows to six or seven cells when space permits, and leaves increasing side margins beyond the cap for a deliberate component-panel look.
- **Do not merge or resample bands:** maintain `[BandState; 10]` and render exactly ten columns in the stable order `31`, `63`, `125`, `250`, `500`, `1k`, `2k`, `4k`, `8k`, `16k`. Width adaptation changes presentation only, never band count or band identity.
- **Labels:** center one compact ASCII label under each band. Five cells are enough for all proposed labels. Add a second `Hz`/`kHz` caption row only when height allows; width should not trigger longer labels because doing so creates jitter on resize.
- **Vertical geometry:** reserve one label row and, when present, one border row at the bottom. Use every remaining plot row as one discrete segment, with a practical target of 12-18 segment rows. If height is below roughly 10 plot rows, omit intermediate dB annotations before reducing the number of bands.
- **Scale mapping:** normalize a fixed analysis range, for example `-60 dB..0 dB`, once per frame and quantize to `0..segment_rows`. Never normalize against the loudest current band. Apply a small quantization hysteresis (about 0.15-0.25 segment) to prevent a value near a boundary alternating rows.
- **Peak markers:** keep one peak position and timer per band. Draw a centered marker of `max(1, band_width - 2)` cells on the peak row, rather than a full-width cap; this preserves the visible identity of an LED segment and leaves side edges readable. Clip the marker to the plot and hide it only when the band has never received a valid sample.
- **Color zones:** choose colors from vertical level, not frequency. A retro VFD preset can use cyan/light-cyan from 0-65%, yellow/light-yellow from 65-85%, and red/light-red from 85-100%. A conventional LED preset can substitute green/light-green in the lower zone. Keep thresholds fixed as rows change so the warning zone retains its level meaning.
- **Color capability:** make named ANSI colors the compatibility baseline (`Cyan`, `LightYellow`, `LightRed`, `DarkGray`, `White`). Offer indexed/RGB palettes only as opt-in or after capability detection. Do not make true color necessary for legibility.
- **Off segments:** render the same `▄` geometry in a subdued color, preferably `DarkGray` in the ANSI palette or a low-luminance cyan-gray in true color. This gives the analyzer a physical unlit display grid and prevents a quiet signal from looking like missing layout.
- **Frame output:** let Ratatui diff old and new buffers and avoid manually clearing/redrawing the terminal. Where the backend and emulator support synchronized updates, make that an optional anti-tearing enhancement, not a requirement; CAVA notes that behavior across terminals is not uniform.

### Gaps
- There is no reliable universal terminal capability query for exact glyph artwork, perceived ANSI color brightness, or synchronized-update behavior. Theme presets and explicit compatibility switches remain necessary.
- The 72-column requirement does not specify a minimum terminal height. The horizontal design is exact, but implementations still need a documented minimum height or a reduced-height mode.

## What animation smoothing values avoid both flicker and sluggishness?

### Takeaway
Decouple audio sampling from terminal painting, smooth each of the ten bands independently with fast attack and slower release, and use a separate hold-and-decay peak state. A strong starting point is a 30 FPS terminal render loop, 40-50 ms attack, 200-250 ms release, 550 ms peak hold, and 14 dB/s peak decay, all calculated from elapsed time rather than frame count.

### Cited Findings
- The Web Audio `AnalyserNode.smoothingTimeConstant` is an average with the previous analysis frame; its valid range is 0-1, zero means no time averaging, and its default is 0.8. MDN says zero produces noticeably more jarring changes and demonstrates 0.85 for a Winamp-style bar visualization. [Source](https://developer.mozilla.org/en-US/docs/Web/API/AnalyserNode/smoothingTimeConstant)
- The Web Audio specification defines temporal smoothing as `smooth = smoothingTimeConstant * previous + (1 - smoothingTimeConstant) * current` after FFT magnitude processing. [Source](https://webaudio.github.io/web-audio-api/#fft-windowing-and-smoothing-over-time)
- CAVA defaults to 60 FPS and a noise-reduction setting of 77 on a 0-100 scale; its comments explicitly characterize 100 as slow/smooth and zero as fast/noisy. [Source](https://raw.githubusercontent.com/karlstav/cava/master/example_files/config)
- CAVA's terminal output is designed to update only changes between frames, supporting the practical strategy of limiting terminal writes independently of the audio-analysis rate. [Source](https://raw.githubusercontent.com/karlstav/cava/master/example_files/config)

### Inferences
- **Use asymmetric envelope smoothing:** for every band, update in normalized dB space with `alpha = 1 - exp(-dt / tau)` and `shown += alpha * (target - shown)`. Select `tau_attack = 45 ms` when `target > shown`, and `tau_release = 220 ms` otherwise. Fast attack preserves drum transients; the roughly five-times-slower release prevents row chatter without making the display hang.
- **Equivalent per-frame values:** at 60 updates/s, the recommended current-sample weights are approximately 0.31 attack and 0.073 release; at 30 updates/s they are approximately 0.52 and 0.14. Computing from `dt` is preferable because the visual response remains stable through stalls and frame-rate changes.
- **Do not blindly stack smoothers:** if the upstream analyzer already applies a Web Audio-style coefficient near 0.8-0.85, reduce or disable UI attack/release smoothing first. Two strong low-pass stages can make the display visibly late even though each stage looks reasonable alone.
- **Separate rates:** ingest/analyze audio at its natural callback cadence, update meter state whenever new values arrive, and render the terminal at 30 FPS by default. Permit 60 FPS as a high-motion option. Thirty FPS is usually enough because segment quantization produces far fewer meaningful states than a pixel display, while Ratatui's diff limits unchanged output.
- **Peak algorithm:** on a new maximum, set `peak = max(peak, shown)` and reset a 550 ms hold timer. After the hold, lower the peak by `14 dB * dt_seconds` until it meets the smoothed band, then follow the band. This creates the recognizable hardware peak-hold behavior without a second EMA that can float indefinitely.
- **Tuning envelope:** expose three presets rather than an unconstrained smoothing scalar: `fast` = 25/140 ms attack/release, 350 ms hold; `balanced` = 45/220 ms, 550 ms hold; `calm` = 70/350 ms, 750 ms hold. Keep peak decay in the approximate 12-18 dB/s range across presets. These are design starting points, not standards, and should be listening-tested with percussion, sustained tones, silence, and resize/load stalls.
- **Quantize after smoothing:** smooth continuous values first, then map to segment rows with hysteresis. Smoothing already-quantized row counts creates step lag and cannot recover sub-row motion.
- **Silence behavior:** after an input timeout, release toward the floor using the normal release constant rather than zeroing all ten bands in one frame. Reset stale peaks after they decay to the floor.

### Gaps
- No cited source establishes universally optimal attack, release, peak-hold, or decay values for a terminal spectrum analyzer. The numeric recommendations above are engineering defaults derived from the sourced smoothing model and terminal update constraints; they require validation against TorroEQ's analyzer cadence, dB range, and target terminals.
- Web Audio's single symmetric smoothing coefficient is useful evidence for temporal averaging but does not specify asymmetric meter ballistics or peak-hold behavior. Those are deliberately separate design recommendations.
