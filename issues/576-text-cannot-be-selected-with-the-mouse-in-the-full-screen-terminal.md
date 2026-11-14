# 576 — Text cannot be selected with the mouse in the full-screen terminal

**Status:** open
**Type:** bug
**Priority:** medium
**Area:** User interaction
**Summary:** `caelumora-tui`'s full-screen view captures the mouse to scroll with its wheel, so selecting text by dragging, copying it and pasting it with the middle button - as every terminal on Linux does - works only with Shift held, which nobody expects.

## The problem

`FullScreen` (`caelumora-tui/src/screen/full/mod.rs`, issue
[569](569-the-terminal-mixes-its-output-into-the-line-typed.md)) enables mouse capture, so that
the wheel scrolls its own scrollback. While the mouse is captured, the terminal hands every click
and drag to the program instead of selecting text: selecting, the primary selection and pasting it
with the middle button work only with Shift held - as the README says, but against what the
user is used to.

## Considerations

*   **No capture.** The terminal selects and pastes as always. The wheel is then the terminal's:
    in the alternate screen many terminals turn it into arrow keys (alternate scroll mode), which
    the view would have to take as scrolling - while the arrows also go through the lines typed
    before.
*   **Capture, and select itself.** The view marks what is dragged over, and copies it - to the
    clipboard by OSC 52, which not every terminal allows, and to the primary selection where the
    terminal supports it -, and takes a middle click as pasting. Much to build, and terminals
    differ.
*   **Capture on demand.** Off by default, so that the terminal selects as always; a key turns it on
    for scrolling with the wheel, or the wheel's arrow keys scroll while nothing is typed.
*   Whatever is chosen, the README says how to select.

## Done when

- Text of the full-screen view is selected, copied and pasted with the mouse as the terminal does
  it elsewhere, and the view can still be scrolled.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal.
