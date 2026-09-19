# Interface studies

These studies translate the TorroMail visual language into a terminal-native audio tool. They are composition references, not pixel-perfect promises: Ratatui will adapt each region to terminal cells and available color depth.

## Shared language

- Torro red brand bar and focused controls
- Quiet charcoal surfaces that allow the host terminal to remain visible
- Numeric values aligned and rendered with monospaced digits
- Status represented with text and symbols as well as color
- Persistent key hints along the bottom edge
- Red left rail or border for selection

## A: Studio (recommended)

[`studio.svg`](studio.svg) balances all live information. A compact session rail exposes device, preset, preamp, bypass, and headroom. The main area gives the analyzer enough height while retaining tactile ten-band faders. This should be the default wide-terminal layout.

## B: Focus

[`focus.svg`](focus.svg) devotes most of the canvas to the analyzer and presents one selected band as a detailed control strip. It is strongest for inspection and small adjustments, but slower for shaping several bands. It is a good optional view rather than the default.

## C: Classic rack

[`classic-rack.svg`](classic-rack.svg) references physical graphic equalizers with compact meters and strong symmetry. It is distinctive and dense, but offers less room for device and preset workflows. Elements of its meter treatment can inform Studio without adopting the entire layout.

## Recommendation

Build Studio first. Preserve Focus as a view toggled with `v`. Borrow the stereo meter and bypass treatment from Classic rack. At widths below 100 columns, collapse the session rail into a modal and keep the analyzer, faders, clipping state, and footer visible.
