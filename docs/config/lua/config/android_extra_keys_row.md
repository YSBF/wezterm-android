# `android_extra_keys_row = true`

*Since: nightly builds only*

When `true`, wezterm draws a row of extra keys along the bottom edge of the
window: `ESC`, `CTRL`, `ALT`, `SHIFT`, `TAB`, arrow keys, `PGUP`/`PGDN`, and a
button that shows or hides the soft keyboard.

Defaults to `true` on Android and `false` everywhere else.

An Android soft keyboard delivers text through an input method, and input
methods carry no modifier information — there is no way for one to tell an
application that Ctrl was held. Without this row there is no `Ctrl-C`, no
`Ctrl-D` and no `Alt-.`, which rules out most of what a terminal is for.

The modifier keys *latch* rather than repeat:

* Tapping `CTRL` arms it, and it is drawn using your cursor colours to show
  that it is active.
* The next key press consumes it, wherever that key came from — this row, the
  soft keyboard, or an attached physical keyboard.
* Tapping it again disarms it.

Several modifiers can be armed at once, so `CTRL` then `ALT` then `x` sends
`Ctrl-Alt-x`.

The row occupies space at the bottom of the window that would otherwise be
available to the terminal grid, so if you always use a Bluetooth or USB
keyboard you may prefer to reclaim it:

```lua
config.android_extra_keys_row = false
```

The soft keyboard is never raised automatically; the row's keyboard button is
what shows and hides it. Turning this row off on a device with no physical
keyboard therefore leaves no way to bring the soft keyboard up.
