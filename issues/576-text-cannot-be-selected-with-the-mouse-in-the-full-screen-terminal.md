# 576 — Text cannot be selected with the mouse in the full-screen terminal

**Status:** in progress
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

No mouse mode of a terminal reports the wheel and clicks while leaving dragging and the middle
button to the terminal: once presses are reported (`?1000h`), the terminal selects nothing. So
either the terminal does all - without capture, where in the alternate screen the wheel arrives as
arrow keys, which the line typed and the choices take - or the program does all.

Claude Code (2.1.322) offers both: its classic way of drawing writes into the terminal's own
scrollback, without the alternate screen, and leaves the mouse to the terminal; its full-screen way
captures the mouse and does all itself - dragging marks, releasing copies to the clipboard and the
primary selection (by `wl-copy`, `xclip` or `xsel`, else by OSC 52), a middle click pastes the
primary selection, and clicks reach what is shown.

Decided (2026-11-15): `full` keeps capturing the mouse and selects itself, as Claude Code's
full-screen way does; the keys stay as they are. The clipboard and the primary selection are reached
through the crate `arboard`, wherever there is a display; where there is none - over SSH -, text is
copied by OSC 52. A click on one of the choices of what is asked chooses it. Steps, each reviewed on
its own:

1.  Dragging marks what it passes over in the window, and releasing copies it to the clipboard and
    the primary selection.
2.  A middle click pastes the primary selection into the line typed.
3.  A click on a choice chooses it; where several may be taken, it marks or unmarks it.

How it feels is judged after the steps: scrolling while dragging past the edge, a double click for
a word, soft-wrapped lines copied as one, and a notice that something was copied are left out until
then.

## Done when

- Text of the full-screen view is selected, copied and pasted with the mouse as the terminal does
  it elsewhere, and the view can still be scrolled.

## History

**2026-11-14** - Opened from the maintainer's use of the terminal.

**2026-11-15** - Decided with the maintainer: `full` selects itself, as Claude Code's full-screen
way does, through `arboard` and OSC 52; a click chooses among what is asked. Three steps.

**2026-11-15** - Step 1 done: dragging with the left button marks cells of the window, shown
reversed, and releasing copies their text as drawn to the clipboard and the primary selection
(`Selection`, `Clipboard`); a key, a paste, a new press, the wheel or a resize drops the marking.
